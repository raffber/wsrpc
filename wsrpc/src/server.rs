use uuid::Uuid;

use tokio::sync::RwLock;
use std::sync::Arc;
use std::collections::{HashMap, HashSet};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::ToSocketAddrs;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::accept_async;
use futures::StreamExt;
use crate::{Message, Response, Request};
use log::{info, debug};
use tokio::task;
use futures::SinkExt;
use serde::de::DeserializeOwned;


#[derive(Clone)]
enum ResponseMsg<Req: Message, Resp: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Response<Resp>),
    Request(Request<Req>),
}

struct ServerShared<Req: Message, Resp: Message> {
    connections: HashMap<Uuid, UnboundedSender<ResponseMsg<Req, Resp>>>,
}

#[derive(Clone)]
pub struct Server<Req: Message, Resp: Message> {
    inner: Arc<RwLock<ServerShared<Req, Resp>>>,
}

pub struct Reply<Req: Message, Resp: Message> {
    id: Uuid,
    server: Server<Req, Resp>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Reply<Req, Resp> {
    pub fn answer(self, resp: Resp) {
        let resp = Response::Reply {
            request: self.id,
            message: resp,
        };
        task::spawn(async move {
            self.server.broadcast_internal(ResponseMsg::Message(resp)).await;
        });
    }
}

pub struct Requested<Req: Message, Resp: Message> {
    msg: Req,
    id: Uuid,
    server: Server<Req, Resp>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Requested<Req, Resp> {
    pub fn answer(self, resp: Resp) {
        let resp = Response::Reply {
            request: self.id,
            message: resp,
        };
        task::spawn(async move {
            self.server.broadcast_internal(ResponseMsg::Message(resp)).await;
        });
    }

    pub fn msg(&self) -> &Req {
        &self.msg
    }

    pub fn take(self) -> (Req, Reply<Req, Resp>) {
        (self.msg, Reply {
            id: self.id,
            server: self.server
        })
    }
}


impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Server<Req, Resp> {
    pub fn new() -> Self
    {
        let inner = ServerShared {
            connections: Default::default(),
        };
        Self {
            inner: Arc::new(RwLock::new(inner))
        }
    }

    pub async fn listen<A: ToSocketAddrs>(&self, addr: A) -> UnboundedReceiver<Requested<Req, Resp>>
    {
        info!("WsRpc Server Listening");
        let mut listener = TcpListener::bind(addr).await.expect("Failed to bind");
        let (tx_req, rx_req) = unbounded_channel();

        let server = self.clone();
        task::spawn(async move {
            // TODO: allow shutting down listener
            while let Ok((stream, path)) = listener.accept().await {
                info!("Client connected on: {}", path);
                let (tx_resp, rx_resp) = unbounded_channel();
                let id = Uuid::new_v4();
                let mut write = server.inner.write().await;
                write.connections.insert(id, tx_resp.clone());
                server.clone().run_client(id, stream, rx_resp, tx_resp, tx_req.clone()).await;
            }
        });

        rx_req
    }

    async fn run_client(self, id: Uuid, stream: TcpStream,
                        mut rx_resp: UnboundedReceiver<ResponseMsg<Req, Resp>>,
                        tx_resp: UnboundedSender<ResponseMsg<Req, Resp>>,
                        tx_req: UnboundedSender<Requested<Req, Resp>>) {
        let ws_stream = accept_async(stream).await.unwrap();
        let (mut write, mut read) = ws_stream.split();
        let server = self.clone();
        task::spawn(async move {
            // TODO: regular pings to detect drops
            while let Some(x) = rx_resp.recv().await {
                match x {
                    ResponseMsg::Pong(x) => {
                        if let Err(_) = write.send(tungstenite::Message::Pong(x)).await {
                            break;
                        }
                    }
                    ResponseMsg::Drop => break,
                    ResponseMsg::Message(msg) => {
                        let data = serde_json::to_string(&msg).unwrap();
                        if let Err(_) = write.send(tungstenite::Message::Text(data)).await {
                            break;
                        }
                    }
                    ResponseMsg::Request(msg) => {
                        let data = serde_json::to_string(&msg).unwrap();
                        if let Err(_) = write.send(tungstenite::Message::Text(data)).await {
                            break;
                        }
                    }
                }
            }
            server.remove_client(&id).await;
            let _ = write.close().await;
            info!("Dropping sender of client: {}", id);
        });
        let server = self.clone();
        task::spawn(async move {
            while let Some(msg) = read.next().await {
                if !server.handle_rx(id, msg, &tx_resp, &tx_req.clone()).await {
                    break;
                }
            }
            info!("Dropping receiver of client: {}", id);
            server.remove_client(&id).await;
        });
    }

    async fn handle_rx(&self, client_id: Uuid,
                       msg: Result<tungstenite::Message, tungstenite::Error>,
                       tx_resp: &UnboundedSender<ResponseMsg<Req, Resp>>,
                       tx_req: &UnboundedSender<Requested<Req, Resp>>,
    ) -> bool {
        match msg {
            Ok(msg) => {
                match msg {
                    tungstenite::Message::Text(text) => {
                        debug!("Message received from client{}: `{}`", client_id, text);
                        match serde_json::from_str::<Request<Req>>(&text) {
                            Ok(msg) => {
                                // we drop in case the receiver of the requests drops
                                self.handle_valid_msg(msg, tx_req.clone()).await
                            }
                            Err(err) => {
                                self.handle_invalid_msg(&client_id, err).await;
                                true
                            }
                        }
                    }
                    tungstenite::Message::Ping(data) =>
                        tx_resp.send(ResponseMsg::Pong(data)).is_ok(),
                    tungstenite::Message::Close(_) => false,
                    _ => true
                }
            }
            Err(_) => false
        }
    }

    async fn handle_valid_msg(&self, msg: Request<Req>, tx_req: UnboundedSender<Requested<Req, Resp>>) -> bool {
        let server = self.clone();
        let req = msg.clone();
        let id = msg.id;
        server.broadcast_internal(ResponseMsg::Request(req)).await;
        tx_req.send(Requested {
            msg: msg.message,
            id,
            server,
        }).is_ok()
    }

    async fn handle_invalid_msg(&self, client_id: &Uuid, err: serde_json::Error) {
        info!("Cannot parse message: {}", err);
        let msg = Response::Error(err.to_string());
        self.send(client_id, msg).await;
    }

    async fn send(&self, id: &Uuid, msg: Response<Resp>) {
        let mut write = self.inner.write().await;
        if let Some(con) = write.connections.get(id) {
            if let Err(_) = con.send(ResponseMsg::Message(msg)) {
                write.connections.remove(id);
            }
        }
    }

    pub async fn broadcast(&self, resp: Resp) {
        let msg = ResponseMsg::Message(Response::Broadcast(resp));
        let server = self.clone();
        task::spawn(async move {
            server.broadcast_internal(msg).await;
        });
    }

    async fn broadcast_internal(&self, resp: ResponseMsg<Req, Resp>) {
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

    pub async fn close_all(&self) {
        info!("Closing all client connections.");
        let mut write = self.inner.write().await;
        for (_id, con) in &write.connections {
            let _ = con.send(ResponseMsg::Drop);
        }
        write.connections.clear();
    }

    async fn remove_client(&self, id: &Uuid) {
        let mut write = self.inner.write().await;
        if let Some(client) = write.connections.remove(&id) {
            let _ = client.send(ResponseMsg::Drop);
        }
    }
}
