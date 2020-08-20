use uuid::Uuid;

use std::future::Future;
use tokio::sync::RwLock;
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, HashSet};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::ToSocketAddrs;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::accept_async;
use futures::StreamExt;
use crate::{Message, Response, Request};
use log::{info, debug};
use tokio::task;
use std::pin::Pin;
use futures::SinkExt;
use serde::de::DeserializeOwned;

type FutureResponse<Resp> =  Pin<Box<dyn 'static + Send + Future<Output=Option<Resp>>>>;
type Handler<Req, Resp> = dyn 'static + Send + Fn(Server<Req, Resp>, Req) -> FutureResponse<Resp>;

struct RequestHandler<Req: Message, Resp: Message> {
    inner: Arc<Mutex<Handler<Req, Resp>>>,
}

impl<Req: Message, Resp: Message> RequestHandler<Req, Resp> {
    fn new<Fut, Fun>(handler: Fun) -> Self
        where
            Fut: 'static + Send + Future<Output=Option<Resp>>,
            Fun: 'static + Send + Fn(Server<Req, Resp>, Req) -> Fut
    {
        Self {
            inner: Arc::new(Mutex::new(move |x, y| {
                let fut: Fut = handler(x, y);
                let ret: FutureResponse<Resp> = Box::pin(fut);
                ret
            }))
        }
    }

    fn call(&self, server: Server<Req, Resp>, req: Req) -> FutureResponse<Resp> {
        let inner = self.inner.lock().unwrap();
        (*inner)(server, req)
    }
}

enum ResponseMsg<M: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Response<M>),
}

pub struct ServerShared<Req: Message, Resp: Message> {
    connections: HashMap<Uuid, UnboundedSender<ResponseMsg<Resp>>>,
    handler: RequestHandler<Req, Resp>,
}

#[derive(Clone)]
pub struct Server<Req: Message, Resp: Message> {
    inner: Arc<RwLock<ServerShared<Req, Resp>>>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message> Server<Req, Resp> {
    pub(crate) fn new<Fut, Fun>(fun: Fun) -> Self
        where
            Fut: 'static + Send + Future<Output=Option<Resp>>,
            Fun: 'static + Send + Fn(Server<Req, Resp>, Req) -> Fut
        {
        let handler = RequestHandler::new(fun);
        let inner = ServerShared {
            connections: Default::default(),
            handler
        };
        Self{
            inner: Arc::new(RwLock::new(inner))
        }

    }

    pub async fn listen<A: ToSocketAddrs>(&self, addr: A) {
        info!("WsRpc Server Listening");
        let mut listener = TcpListener::bind(addr).await.expect("Failed to bind");

        // TODO: allow shutting down listener
        while let Ok((stream, path)) = listener.accept().await {
            info!("Client connected on: {}", path);
            let (tx_resp, rx_resp) = unbounded_channel();
            let id = Uuid::new_v4();
            let mut write = self.inner.write().await;
            write.connections.insert(id, tx_resp.clone());
            self.clone().run_client(id, stream, rx_resp, tx_resp).await;
        }
    }

    async fn run_client(self, id: Uuid, stream: TcpStream, mut rx_resp: UnboundedReceiver<ResponseMsg<Resp>>, tx_resp: UnboundedSender<ResponseMsg<Resp>>) {
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
                }
            }
            server.remove_client(&id).await;
            let _ = write.close().await;
            info!("Dropping sender of client: {}", id);
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

    async fn handle_rx(&self, client_id: Uuid, msg: Result<tungstenite::Message, tungstenite::Error>,
                       tx_resp: &UnboundedSender<ResponseMsg<Resp>>) -> bool {
        match msg {
            Ok(msg) => {
                match msg {
                    tungstenite::Message::Text(text) => {
                        debug!("Message received from client{}: `{}`", client_id, text);
                        match serde_json::from_str::<Request<Req>>(&text) {
                            Ok(msg) => {
                                self.handle_valid_msg(msg).await;
                                true
                            }
                            Err(err) => {
                                self.handle_invalid_msg(&client_id, err).await;
                                true
                            }
                        }
                    }
                    tungstenite::Message::Ping(data) => tx_resp.send(ResponseMsg::Pong(data)).is_ok(),
                    tungstenite::Message::Close(_) => false,
                    _ => true
                }
            }
            Err(_) => false
        }
    }

    async fn handle_valid_msg(&self, msg: Request<Req>) {
        let server = self.clone();
        let id = msg.id;
        let reply = {
            let read = server.inner.read().await;
            read.handler.call(server.clone(), msg.message)
        };
        task::spawn(async move {
            if let Some(reply) = reply.await {
                let resp = Response::Success {
                    request: id,
                    message: reply,
                };
                server.broadcast(resp).await;
            }
        });
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

    async fn broadcast(&self, resp: Response<Resp>) {
        debug!("Broadcasting: {}", serde_json::to_string(&resp).unwrap());
        let mut to_remove = HashSet::new();
        {
            let read = self.inner.read().await;
            for (id, con) in &read.connections {
                if let Err(_) = con.send(ResponseMsg::Message(resp.clone())) {
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

    pub async fn remove_client(&self, id: &Uuid) {
        let mut write = self.inner.write().await;
        if let Some(client) = write.connections.remove(&id) {
            let _ = client.send(ResponseMsg::Drop);
        }
    }
}
