use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use futures::stream::SplitSink;
use futures::SinkExt;
use log::{debug, info};
use serde::de::DeserializeOwned;
use tokio::net::ToSocketAddrs;
use tokio::net::{TcpListener, TcpStream};
use tokio::stream::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::sync::RwLock;
use tokio::task;
use tokio::time;
use tokio::time::Duration;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::WebSocketStream;
use uuid::Uuid;

use crate::http::HttpServer;
use crate::{Message, Request, Response};

#[derive(Clone)]
pub(crate) enum SenderMsg<Req: Message, Resp: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Response<Req, Resp>),
}

struct ServerShared<Req: Message, Resp: Message> {
    connections: HashMap<Uuid, UnboundedSender<SenderMsg<Req, Resp>>>,
}

#[derive(Clone)]
pub struct Server<Req: Message, Resp: Message> {
    inner: Arc<RwLock<ServerShared<Req, Resp>>>,
    tx_req: UnboundedSender<Requested<Req, Resp>>,
}

pub struct Reply<Req: Message, Resp: Message> {
    pub(crate) id: Uuid,
    pub(crate) server: Server<Req, Resp>,
    pub(crate) direct: Option<oneshot::Sender<Resp>>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Reply<Req, Resp> {
    pub fn answer(mut self, resp: Resp) {
        if let Some(direct) = self.direct.take() {
            let _ = direct.send(resp.clone());
        }
        let resp = Response::Reply {
            request: self.id.clone(),
            message: resp,
        };
        task::spawn(async move {
            self.server
                .broadcast_internal(SenderMsg::Message(resp))
                .await;
        });
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
        };
        let server = Self {
            inner: Arc::new(RwLock::new(inner)),
            tx_req,
        };
        (server, rx_req)
    }

    pub async fn listen<A: ToSocketAddrs>(&self, addr: A, http_addr: SocketAddr) {
        info!("WsRpc Server Listening");
        let mut listener = TcpListener::bind(addr).await.expect("Failed to bind");

        HttpServer::spawn(http_addr, self.clone(), self.tx_req.clone());

        let server = self.clone();
        task::spawn(async move {
            while let Ok((stream, path)) = listener.accept().await {
                info!("Client connected on: {}", path);
                let (tx_resp, rx_resp) = unbounded_channel();
                let id = Uuid::new_v4();
                let mut write = server.inner.write().await;
                write.connections.insert(id, tx_resp.clone());
                server
                    .clone()
                    .run_client(id, stream, rx_resp, tx_resp)
                    .await;
            }
        });
    }

    pub async fn loopback(&self) -> LoopbackClient<Req, Resp> {
        let (client_tx, mut client_rx) = unbounded_channel::<Request<Req>>();
        let (sender_tx, mut sender_rx) = unbounded_channel();
        let (network_tx, network_rx) = unbounded_channel();

        {
            let mut write = self.inner.write().await;
            write.connections.insert(Uuid::new_v4(), sender_tx);
        }

        let server = self.clone();
        task::spawn(async move {
            while let Some(msg) = client_rx.recv().await {
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
        let ws_stream = accept_async(stream).await.unwrap();
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
            server.remove_client(&id).await;
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
            server.remove_client(&id).await;
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
                    if let Err(_) = write.send(tungstenite::Message::Text(data)).await {
                        return false;
                    }
                }
            },
        }
        true
    }

    async fn handle_rx(
        &self,
        client_id: Uuid,
        msg: Result<tungstenite::Message, tungstenite::Error>,
        tx_resp: &UnboundedSender<SenderMsg<Req, Resp>>,
    ) -> bool {
        match msg {
            Ok(msg) => {
                match msg {
                    tungstenite::Message::Text(text) => {
                        debug!("Message received: `{}`", text);
                        match serde_json::from_str::<Request<Req>>(&text) {
                            Ok(msg) => {
                                // we drop in case the receiver of the requests drops
                                self.handle_valid_msg(msg).await
                            }
                            Err(err) => {
                                self.handle_invalid_msg(&client_id, err).await;
                                true
                            }
                        }
                    }
                    tungstenite::Message::Ping(data) => tx_resp.send(SenderMsg::Pong(data)).is_ok(),
                    tungstenite::Message::Close(_) => false,
                    _ => true,
                }
            }
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
        server
            .broadcast_internal(SenderMsg::Message(broadcast))
            .await;
        self.tx_req
            .send(Requested {
                msg: msg.message,
                reply: Reply {
                    id,
                    server,
                    direct: None,
                },
            })
            .is_ok()
    }

    async fn handle_invalid_msg(&self, client_id: &Uuid, err: serde_json::Error) {
        info!("Cannot parse message: {}", err);
        let msg = Response::Error(err.to_string());
        self.send(client_id, msg).await;
    }

    async fn send(&self, id: &Uuid, msg: Response<Req, Resp>) {
        let mut write = self.inner.write().await;
        if let Some(con) = write.connections.get(id) {
            if let Err(_) = con.send(SenderMsg::Message(msg)) {
                write.connections.remove(id);
            }
        }
    }

    pub async fn broadcast(&self, resp: Resp) {
        let msg = SenderMsg::Message(Response::Notify(resp));
        let server = self.clone();
        task::spawn(async move {
            server.broadcast_internal(msg).await;
        });
    }

    pub(crate) async fn broadcast_internal(&self, resp: SenderMsg<Req, Resp>) {
        let mut to_remove = HashSet::new();
        {
            let read = self.inner.read().await;
            for (id, con) in &read.connections {
                if let Err(_) = con.send(resp.clone()) {
                    to_remove.insert(*id);
                }
            }
        }
        for x in &to_remove {
            self.remove_client(x).await;
        }
    }

    pub async fn shutdown(&self) {
        info!("Closing all client connections.");
        let mut write = self.inner.write().await;
        for (_id, con) in &write.connections {
            let _ = con.send(SenderMsg::Drop);
        }
        write.connections.clear();
    }

    async fn remove_client(&self, id: &Uuid) {
        let mut write = self.inner.write().await;
        if let Some(client) = write.connections.remove(&id) {
            let _ = client.send(SenderMsg::Drop);
        }
    }
}
