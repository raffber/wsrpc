use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::RwLock;

use futures::stream::SplitSink;
use futures::SinkExt;
use log::{debug, info};
use serde::de::DeserializeOwned;
use tokio::net::ToSocketAddrs;
use tokio::net::{TcpListener, TcpStream};
use tokio::stream::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::WebSocketStream;
use uuid::Uuid;

use crate::http::HttpServer;
use crate::{Message, Request, Response};

type JsonValue = serde_json::Value;

#[derive(Clone)]
pub(crate) enum SenderMsg<Req: Message, Resp: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Response<Req, Resp>),
}

struct ServerShared<Req: Message, Resp: Message> {
    connections: HashMap<Uuid, UnboundedSender<SenderMsg<Req, Resp>>>,
    abort_handles: Vec<oneshot::Sender<()>>,
    tx_req: Option<UnboundedSender<Requested<Req, Resp>>>,
    broadcast_reqrep: bool,
}

#[derive(Clone)]
pub struct Server<Req: Message, Resp: Message> {
    inner: Arc<RwLock<ServerShared<Req, Resp>>>,
}

pub struct Reply<Req: Message, Resp: Message> {
    pub(crate) id: Uuid,
    pub(crate) server: Server<Req, Resp>,
    pub(crate) direct: Option<oneshot::Sender<Resp>>,
    pub(crate) sender: Option<Uuid>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Reply<Req, Resp> {
    pub fn answer(self, resp: Resp) {
        let broadcast = { self.server.inner.read().unwrap().broadcast_reqrep };
        if broadcast {
            self.answer_broadcast(resp);
        } else {
            self.answer_unicast(resp);
        }
    }

    pub fn answer_unicast(mut self, resp: Resp) {
        if let Some(direct) = self.direct.take() {
            let _ = direct.send(resp.clone());
        }
        let resp = Response::Reply {
            request: self.id.clone(),
            message: resp,
            sender: self.sender,
        };
        let sender = self.sender.clone();
        if let Some(sender) = sender {
            self.server.unicast(&sender, SenderMsg::Message(resp));
        }
    }

    pub fn answer_broadcast(mut self, resp: Resp) {
        if let Some(direct) = self.direct.take() {
            let _ = direct.send(resp.clone());
        }
        let resp = Response::Reply {
            request: self.id.clone(),
            message: resp,
            sender: self.sender,
        };
        self.server.broadcast_internal(SenderMsg::Message(resp));
    }
}

pub struct Requested<Req: Message, Resp: Message> {
    pub(crate) msg: Req,
    pub(crate) reply: Reply<Req, Resp>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Requested<Req, Resp> {
    pub fn answer(self, resp: Resp) {
        self.reply.answer(resp);
    }

    pub fn msg(&self) -> &Req {
        &self.msg
    }

    pub fn split(self) -> (Req, Reply<Req, Resp>) {
        (self.msg, self.reply)
    }
}

enum Merged<Req: Message, Resp: Message> {
    Interval,
    Msg(SenderMsg<Req, Resp>),
}

pub struct LoopbackClient<Req: Message, Resp: Message> {
    rx: UnboundedReceiver<Response<Req, Resp>>,
    tx: UnboundedSender<Request<Req>>,
}

impl<Req: Message, Resp: Message> LoopbackClient<Req, Resp> {
    pub fn sender(&self) -> UnboundedSender<Request<Req>> {
        self.tx.clone()
    }

    pub fn receiver(self) -> UnboundedReceiver<Response<Req, Resp>> {
        self.rx
    }

    pub fn split(
        self,
    ) -> (
        UnboundedSender<Request<Req>>,
        UnboundedReceiver<Response<Req, Resp>>,
    ) {
        (self.tx, self.rx)
    }

    pub async fn next(&mut self) -> Option<Response<Req, Resp>> {
        self.rx.recv().await
    }
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Server<Req, Resp> {
    pub fn new() -> (Self, UnboundedReceiver<Requested<Req, Resp>>) {
        let (tx_req, rx_req) = unbounded_channel();
        let inner = ServerShared {
            connections: Default::default(),
            abort_handles: vec![],
            tx_req: Some(tx_req),
            broadcast_reqrep: false,
        };
        let server = Self {
            inner: Arc::new(RwLock::new(inner)),
        };
        (server, rx_req)
    }

    pub fn enable_broadcast_reqrep(&self, enable_reqrep: bool) {
        let mut write = self.inner.write().unwrap();
        write.broadcast_reqrep = enable_reqrep;
    }

    pub async fn listen_ws<A: ToSocketAddrs>(&self, addr: A) {
        info!("WsRpc Server Listening");
        let mut listener = TcpListener::bind(addr).await.expect("Failed to bind");

        let (cancel_tx, cancel_rx) = oneshot::channel();
        {
            let mut write = self.inner.write().unwrap();
            write.abort_handles.push(cancel_tx);
        }

        let server = self.clone();
        task::spawn(async move {
            let accept_loop = async move {
                loop {
                    server.ws_accept(&mut listener).await
                }
            };
            tokio::select! {
                _ = accept_loop => {},
                _ = cancel_rx => {},
            }
            debug!("Dropping ws listener.");
        });
    }

    pub async fn dispatch_request(&self, req: Requested<Req, Resp>) -> bool {
        let read = self.inner.read().unwrap();
        if let Some(tx) = &read.tx_req {
            tx.send(req).is_ok()
        } else {
            false
        }
    }

    async fn ws_accept(&self, listener: &mut TcpListener) {
        if let Ok((stream, path)) = listener.accept().await {
            info!("Client connected on: {}", path);
            let (tx_resp, rx_resp) = unbounded_channel();
            let id = Uuid::new_v4();
            {
                let mut write = self.inner.write().unwrap();
                write.connections.insert(id, tx_resp.clone());
            }
            self.clone().run_client(id, stream, rx_resp, tx_resp).await;
        }
    }

    pub async fn listen_http<T: Into<SocketAddr>>(&self, http_addr: T) {
        let abort_handle = HttpServer::spawn(http_addr, self.clone());
        let mut write = self.inner.write().unwrap();
        write.abort_handles.push(abort_handle);
    }

    pub async fn loopback(&self) -> LoopbackClient<Req, Resp> {
        let (client_tx, mut client_rx) = unbounded_channel::<Request<Req>>();
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

        let client = LoopbackClient {
            rx: network_rx,
            tx: client_tx,
        };
        client
    }

    async fn run_client(
        self,
        id: Uuid,
        stream: TcpStream,
        rx_resp: UnboundedReceiver<SenderMsg<Req, Resp>>,
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
            let interval = time::interval(Duration::from_millis(500)).map(|_| Merged::Interval);
            let rx = rx_resp.map(Merged::Msg);
            let mut merged = interval.merge(rx);
            while let Some(msg) = merged.next().await {
                if !server.write(&mut write, msg).await {
                    break;
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
        write: &mut SplitSink<WebSocketStream<TcpStream>, tungstenite::Message>,
        msg: Merged<Req, Resp>,
    ) -> bool {
        match msg {
            Merged::Interval => {
                if let Err(_) = write
                    .send(tungstenite::Message::Ping(vec![1, 2, 3, 4]))
                    .await
                {
                    return false;
                }
            }
            Merged::Msg(x) => match x {
                SenderMsg::Pong(x) => {
                    if let Err(_) = write.send(tungstenite::Message::Pong(x)).await {
                        return false;
                    }
                }
                SenderMsg::Drop => return false,
                SenderMsg::Message(msg) => {
                    let data = serde_json::to_string(&msg).unwrap();
                    log::debug!("Sending: {}", data);
                    if let Err(_) = write.send(tungstenite::Message::Text(data)).await {
                        return false;
                    }
                }
            },
        }
        true
    }

    async fn handle_text_message(&self, text: &str, client_id: Uuid) -> bool {
        debug!("Message received: `{}`", text);
        match serde_json::from_str::<Request<JsonValue>>(&text) {
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
        msg: Result<tungstenite::Message, tungstenite::Error>,
        tx_resp: &UnboundedSender<SenderMsg<Req, Resp>>,
    ) -> bool {
        match msg {
            Ok(msg) => match msg {
                tungstenite::Message::Text(text) => self.handle_text_message(&text, client_id).await,
                tungstenite::Message::Ping(data) => tx_resp.send(SenderMsg::Pong(data)).is_ok(),
                tungstenite::Message::Close(_) => false,
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
        let bc = { self.inner.read().unwrap().broadcast_reqrep };
        if bc {
            server.broadcast_internal(SenderMsg::Message(broadcast));
        }
        self.dispatch_request(Requested {
            msg: msg.message,
            reply: Reply {
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
            if let Err(_) = con.send(SenderMsg::Message(msg)) {
                write.connections.remove(id);
            }
        }
    }

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
                if let Err(_) = con.send(resp.clone()) {
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
                if let Err(_) = con.send(msg) {
                    to_remove.replace(sender);
                }
            }
        }
        if let Some(x) = to_remove {
            self.remove_client(&x);
        }
    }

    pub async fn shutdown(&self) {
        info!("Closing all client connections.");
        let mut write = self.inner.write().unwrap();
        for (_id, con) in &write.connections {
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
        if let Some(client) = write.connections.remove(&id) {
            let _ = client.send(SenderMsg::Drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

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
        server.shutdown().await;
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

        server.shutdown().await;
    }
}
