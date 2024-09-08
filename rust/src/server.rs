//! This module exposes the [`Server`] struct, which allows spawning a server listening on WebSockets, HTTP
//! and a local loopback channel.
//!

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::RwLock;

use async_tungstenite::tokio::{accept_async, TokioAdapter};
use async_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};
use async_tungstenite::WebSocketStream;
use futures::stream::SplitSink;
use futures::SinkExt;
use log::{debug, info};
use serde::de::DeserializeOwned;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::connection::Sender;
use crate::http::HttpServer;
use crate::{Message, Request, Response};

type JsonValue = serde_json::Value;

const MAX_LOG_CHARS: usize = 500;

#[derive(Clone)]
pub(crate) enum SenderMsg<Req: Message, Resp: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Response<Req, Resp>),
}
struct ServerShared<Req: Message, Resp: Message> {
    connections: HashMap<Uuid, Sender>,
    tx_req: UnboundedSender<Requested<Req, Resp>>,
}

///
/// A server which allows listening on WebSockets, HTTP and a local loopback channel.
///
/// ```no_run
/// # use broadcast_wsrpc::server::Server;
/// # use tokio::runtime::Runtime;
/// # use serde::{Serialize, Deserialize};
/// #[derive(Serialize, Deserialize, Clone)]
/// enum Request { Ping }
///
/// #[derive(Serialize, Deserialize, Clone)]
/// enum Response { Pong }
///
/// let rt = Runtime::new().unwrap();
/// rt.block_on(async move {
///     let (mut server, mut rx) = Server::<Request, Response>::new();
///     server.enable_broadcast_reqrep(true); // requests are broadcasted to all clients
///     server.listen_ws(&"0.0.0.0:8000".parse().unwrap()).await;
///     server.listen_http(&"0.0.0.0:8001".parse().unwrap()).await;
///     while let Some(_request) = rx.recv().await {
///         // do something with request
///         // let response = handle(request);
///         // request.answer(response)
///     }
/// });
/// ```
#[derive(Clone)]
pub struct Server<Req: Message, Resp: Message> {
    inner: Arc<RwLock<ServerShared<Req, Resp>>>,
    broadcast_reqrep: Arc<AtomicBool>,
}

/// Allows responding to a specific request.
///
/// This type is obtained by calling `split()` on a [`Requested`] type.
/// The application can decide whether it wants to unicast or broadcast the response or leave
/// it up to the server settings.
pub struct RespondTo<Req: Message, Resp: Message> {
    pub(crate) id: Uuid,
    pub(crate) server: Server<Req, Resp>,
    pub(crate) direct: Option<oneshot::Sender<Resp>>,
    pub(crate) sender: Option<Uuid>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> RespondTo<Req, Resp> {
    /// Reply to the request, either by unicast or broadcast depending on the server settings.
    pub fn answer(self, resp: Resp) {
        let broadcast = self.server.broadcast_reqrep.load(Ordering::SeqCst);
        if broadcast {
            self.answer_broadcast(resp);
        } else {
            self.answer_unicast(resp);
        }
    }

    /// Reply to the request but only sending the response back the client where the request
    /// originated from.
    pub fn answer_unicast(mut self, resp: Resp) {
        if let Some(direct) = self.direct.take() {
            let _ = direct.send(resp.clone());
        }
        let resp = Response::Reply {
            request: self.id,
            message: resp,
            sender: self.sender,
        };
        let sender = self.sender;
        if let Some(sender) = sender {
            self.server.unicast(&sender, SenderMsg::Message(resp));
        }
    }

    /// Reply to the request by broadcasting the response to all client
    pub fn answer_broadcast(mut self, resp: Resp) {
        if let Some(direct) = self.direct.take() {
            let _ = direct.send(resp.clone());
        }
        let resp = Response::Reply {
            request: self.id,
            message: resp,
            sender: self.sender,
        };
        self.server.broadcast_internal(SenderMsg::Message(resp));
    }

    pub fn request_id(&self) -> Uuid {
        self.id
    }
}

/// A type capturing a request and the means to answer the request.
pub struct Requested<Req: Message, Resp: Message> {
    pub(crate) msg: Req,
    pub(crate) reply: RespondTo<Req, Resp>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Requested<Req, Resp> {
    pub fn answer(self, resp: Resp) {
        self.reply.answer(resp);
    }

    pub fn msg(&self) -> &Req {
        &self.msg
    }

    pub fn split(self) -> (Req, RespondTo<Req, Resp>) {
        (self.msg, self.reply)
    }

    pub fn request_id(self) -> Uuid {
        self.reply.id
    }
}

/// A client channel that can be used to loop back requests from the server.
pub struct LoopbackClient<Req: Message, Resp: Message> {
    rx: UnboundedReceiver<Response<Req, Resp>>,
    tx: UnboundedSender<Request<Req>>,
}

impl<Req: Message, Resp: Message> LoopbackClient<Req, Resp> {
    /// Returns a sender that allows sending requests
    pub fn sender(&self) -> UnboundedSender<Request<Req>> {
        self.tx.clone()
    }

    /// Returns a receiver that receives all messages transmitted from the server to the client
    pub fn receiver(self) -> UnboundedReceiver<Response<Req, Resp>> {
        self.rx
    }

    /// Split the client into sender and receiver
    pub fn split(
        self,
    ) -> (
        UnboundedSender<Request<Req>>,
        UnboundedReceiver<Response<Req, Resp>>,
    ) {
        (self.tx, self.rx)
    }

    /// Receive the next message from the server
    pub async fn next(&mut self) -> Option<Response<Req, Resp>> {
        self.rx.recv().await
    }
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Server<Req, Resp> {
    /// Create a new server, returning the server instance and a receiver for the messages it receives.
    ///
    /// The application should listen to the receiver and handle all requests it receives.
    pub fn new() -> (Self, UnboundedReceiver<Requested<Req, Resp>>) {
        let (tx_req, rx_req) = unbounded_channel();
        let inner = ServerShared {
            connections: Default::default(),
            abort_handles: vec![],
            tx_req: Some(tx_req),
        };
        let server = Self {
            inner: Arc::new(RwLock::new(inner)),
            broadcast_reqrep: Arc::new(AtomicBool::new(false)),
        };
        (server, rx_req)
    }

    /// By default enable broadcasting responses to all connected client
    ///
    /// The application may still customize the behavior for each request it handles, by
    /// calling either `RespondTo::answer_unicast()` or `RespondTo::answer_broadcast()`.
    pub fn enable_broadcast_reqrep(&self, enable_reqrep: bool) {
        self.broadcast_reqrep.store(enable_reqrep, Ordering::SeqCst);
    }

    pub fn register_sender(&self, tx: Sender) -> Uuid {
        let mut write = self.inner.write().unwrap();
        let id = Uuid::new_v4();
        write.connections.insert(id, tx);
        id
    }

    pub fn disconnect_sender(&self, id: Uuid) {
        self.remove_client(&id);
    }

    pub(crate) async fn dispatch_request(&self, req: Requested<Req, Resp>) -> bool {
        let read = self.inner.read().unwrap();
        if let Some(tx) = &read.tx_req {
            tx.send(req).is_ok()
        } else {
            false
        }
    }

    /// Create a local [`LoopbackClient`] which allows sending requests without opening any sockets.
    pub async fn loopback(&self) -> LoopbackClient<Req, Resp> {
        let (client_tx, mut client_rx) = unbounded_channel();
        let (sender_tx, mut sender_rx) = unbounded_channel();
        let (network_tx, network_rx) = unbounded_channel();
        let client_id = Uuid::new_v4();
        {
            let mut write = self.inner.write().unwrap();
            write.connections.insert(client_id, sender_tx);
        }

        let server = self.clone();
        task::spawn(async move {
            while let Some(mut msg) = client_rx.recv().await {
                msg.sender = Some(client_id);
                server.handle_valid_msg(msg).await;
            }
        });

        task::spawn(async move {
            while let Some(msg) = sender_rx.recv().await {
                match msg {
                    SenderMsg::Drop => break,
                    SenderMsg::Message(msg) => {
                        if network_tx.send(msg).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });

        LoopbackClient {
            rx: network_rx,
            tx: client_tx,
        }
    }

    async fn run_client(
        self,
        id: Uuid,
        stream: TcpStream,
        mut rx_resp: UnboundedReceiver<SenderMsg<Req, Resp>>,
        tx_resp: UnboundedSender<SenderMsg<Req, Resp>>,
    ) {
        let _ = stream.set_nodelay(true);
        let ws_stream = accept_async(stream).await;
        if ws_stream.is_err() {
            return;
        }
        let ws_stream = ws_stream.unwrap();
        let (mut write, mut read) = futures::StreamExt::split(ws_stream);
        let server = self.clone();
        task::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(500));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if write.send(WsMessage::Ping(vec![1, 2, 3, 4])).await.is_err() {
                            break;
                        }
                    }
                    msg = rx_resp.recv() => {
                        if !server.write(&mut write, msg).await {
                            break;
                        }
                    }
                }
            }

            server.remove_client(&id);
            let _ = write.close().await;
            debug!("Dropping sender of client: {}", id);
        });
        let server = self.clone();
        task::spawn(async move {
            while let Some(msg) = read.next().await {
                if !server.handle_rx(id, msg, &tx_resp).await {
                    break;
                }
            }
            info!("Dropping receiver of client: {}", id);
            server.remove_client(&id);
        });
    }

    async fn write(
        &self,
        write: &mut SplitSink<WebSocketStream<TokioAdapter<TcpStream>>, WsMessage>,
        msg: Option<SenderMsg<Req, Resp>>,
    ) -> bool {
        match msg {
            Some(SenderMsg::Pong(x)) => write.send(WsMessage::Pong(x)).await.is_ok(),
            Some(SenderMsg::Drop) => false,
            Some(SenderMsg::Message(msg)) => {
                let data = serde_json::to_string(&msg).unwrap();
                if data.len() < MAX_LOG_CHARS {
                    log::debug!("Sending: {}", data);
                } else {
                    let x: String = data.chars().take(MAX_LOG_CHARS).collect();
                    log::debug!("Sending: {} ...", x);
                }
                write.send(WsMessage::Text(data)).await.is_ok()
            }
            None => false,
        }
    }

    async fn handle_text_message(&self, text: &str, client_id: Uuid) -> bool {
        if text.len() < MAX_LOG_CHARS {
            log::debug!("Message received: {}", text);
        } else {
            let x: String = text.chars().take(MAX_LOG_CHARS).collect();
            log::debug!("Message received: {} ...", x);
        }
        match serde_json::from_str::<Request<JsonValue>>(text) {
            Ok(msg) => {
                // we drop in case the receiver of the requests drops
                let content = serde_json::from_value::<Req>(msg.message);
                match content {
                    Ok(content) => {
                        let req = Request {
                            id: msg.id,
                            message: content,
                            sender: Some(client_id),
                        };
                        self.handle_valid_msg(req).await
                    }
                    Err(err) => {
                        info!("Invalid Request: {}", err);
                        let msg = Response::InvalidRequest {
                            id: msg.id,
                            description: err.to_string(),
                        };
                        self.send(&client_id, msg).await;
                        true
                    }
                }
            }
            Err(err) => {
                info!("Cannot parse message: {}", err);
                let msg = Response::Error(err.to_string());
                self.send(&client_id, msg).await;
                true
            }
        }
    }

    async fn handle_rx(
        &self,
        client_id: Uuid,
        msg: Result<WsMessage, WsError>,
        tx_resp: &UnboundedSender<SenderMsg<Req, Resp>>,
    ) -> bool {
        match msg {
            Ok(msg) => match msg {
                WsMessage::Text(text) => self.handle_text_message(&text, client_id).await,
                WsMessage::Ping(data) => tx_resp.send(SenderMsg::Pong(data)).is_ok(),
                WsMessage::Close(_) => false,
                _ => true,
            },
            Err(_) => false,
        }
    }

    async fn handle_valid_msg(&self, msg: Request<Req>) -> bool {
        let server = self.clone();
        let id = msg.id;
        let broadcast = Response::Request {
            id: msg.id,
            message: msg.message.clone(),
        };
        let bc = { self.broadcast_reqrep.load(Ordering::SeqCst) };
        if bc {
            server.broadcast_internal(SenderMsg::Message(broadcast));
        }
        self.dispatch_request(Requested {
            msg: msg.message,
            reply: RespondTo {
                id,
                server,
                direct: None,
                sender: msg.sender,
            },
        })
        .await
    }

    async fn send(&self, id: &Uuid, msg: Response<Req, Resp>) {
        let mut write = self.inner.write().unwrap();
        if let Some(con) = write.connections.get(id) {
            if con.send(SenderMsg::Message(msg)).is_err() {
                write.connections.remove(id);
            }
        }
    }

    /// Broadcast a "Notify" message to all clients
    pub fn broadcast(&self, resp: Resp) {
        let msg = SenderMsg::Message(Response::Notify(resp));
        let server = self.clone();
        server.broadcast_internal(msg);
    }

    pub(crate) fn broadcast_internal(&self, resp: SenderMsg<Req, Resp>) {
        let mut to_remove = HashSet::new();
        {
            let read = self.inner.read().unwrap();
            for (id, con) in &read.connections {
                if con.send(resp.clone()).is_err() {
                    to_remove.insert(*id);
                }
            }
        }
        for x in &to_remove {
            self.remove_client(x);
        }
    }

    pub(crate) fn unicast(&self, sender: &Uuid, msg: SenderMsg<Req, Resp>) {
        let mut to_remove = None;
        {
            let read = self.inner.read().unwrap();
            if let Some(con) = read.connections.get(sender) {
                if con.send(msg).is_err() {
                    to_remove.replace(sender);
                }
            }
        }
        if let Some(x) = to_remove {
            self.remove_client(x);
        }
    }

    /// Shutdown the server, disconnecting all clients.
    pub fn shutdown(&self) {
        info!("Closing all client connections.");
        let mut write = self.inner.write().unwrap();
        for con in write.connections.values() {
            let _ = con.send(SenderMsg::Drop);
        }
        write.connections.clear();
        // abort all client handlers
        for x in write.abort_handles.drain(..) {
            let _ = x.send(());
        }
        // drop sender
        let _ = write.tx_req.take();
    }

    fn remove_client(&self, id: &Uuid) {
        let mut write = self.inner.write().unwrap();
        if let Some(client) = write.connections.remove(id) {
            let _ = client.send(SenderMsg::Drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use std::net::SocketAddr;

    #[derive(Clone, Serialize, Deserialize)]
    enum Request {
        Shutdown,
    }

    #[derive(Clone, Serialize, Deserialize)]
    enum Response {
        Done,
    }

    type Server = super::Server<Request, Response>;

    #[tokio::test]
    async fn shutdown_one() {
        let (server, mut rx) = Server::new();

        let client = server.loopback().await;
        let (tx, _) = client.split();
        let _ = tx.send(crate::Request::new(Request::Shutdown, None));

        let msg = rx.recv().await.unwrap();
        let (req, _) = msg.split();
        assert!(matches!(req, Request::Shutdown));
        server.shutdown();
    }

    #[tokio::test]
    async fn shutdown_two() {
        let (server, _) = Server::new();

        let mut client = server.loopback().await;

        server.broadcast(Response::Done);
        let rx_msg = client.next().await.unwrap();
        match rx_msg {
            crate::Response::Notify(msg) => assert!(matches!(msg, Response::Done)),
            _ => panic!(),
        }

        server.shutdown();
    }

    #[tokio::test]
    async fn listen_ws() -> anyhow::Result<()> {
        let (server, _) = Server::new();
        let addr: SocketAddr = "127.0.0.1:11234".parse()?;
        server.listen_ws(&addr).await?;
        server.shutdown();
        Ok(())
    }
}
