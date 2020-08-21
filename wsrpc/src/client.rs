use crate::{Message, Request, Response};
use tokio_tungstenite::{connect_async, WebSocketStream};
use std::time::{Instant, Duration};
use tokio::time::delay_for;
use std::io;
use tokio::io::ErrorKind;
use tungstenite::Error as WsError;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel, UnboundedReceiver};
use tokio::sync::broadcast::{Sender as BcSender, Receiver as BcReceiver, channel as bc_channel};
use tokio::task;
use tokio::net::TcpStream;
use futures::stream::{SplitStream, SplitSink};
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use uuid::Uuid;
use thiserror::Error;
use tokio::time::timeout;
use url::Url;

type WsStream = WebSocketStream<TcpStream>;

const BC_CHANNEL_SIZE: usize = 1000;

pub type LogLevel = i32;

pub type Monitor<Resp> = BcReceiver<Response<Resp>>;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("IO Error occurred: {0}")]
    Io(std::io::Error),
    #[error("Timeout occurred")]
    Timeout,
    #[error("Reciever hung up")]
    ReceiverHungUp,
    #[error("Sender hung up")]
    SenderHungUp,
}

#[derive(Clone)]
enum SenderMsg<Req: Message> {
    Drop,
    Pong(Vec<u8>),
    Message(Request<Req>),
}

#[derive(Clone)]
pub struct Client<Req: Message, Resp: Message> {
    tx: UnboundedSender<SenderMsg<Req>>,
    tx_bc: BcSender<Response<Resp>>,
}

impl<Req: 'static + Message, Resp: 'static + Message + DeserializeOwned> Client<Req, Resp> {
    pub async fn connect<A>(url: A, duration: Duration) -> io::Result<Self>
        where
            A: Into<Url>
    {
        let start = Instant::now();
        let url = url.into();

        loop {
            match connect_async(url.clone()).await {
                Ok((stream, _)) => {
                    return Ok(Self::with_stream(stream));
                }
                Err(WsError::Io(x)) => {
                    // this is fatal
                    return Err(x);
                }
                _ => {}
            }
            if start.elapsed().as_secs_f32() > duration.as_secs_f32() {
                break;
            }
            delay_for(Duration::from_millis(10)).await;
        }
        Err(io::Error::new(ErrorKind::TimedOut, "Timeout during connection attempts."))
    }

    pub fn with_stream(stream: WsStream) -> Self {
        let (write, read) = stream.split();
        let (receiver_tx, _) = bc_channel(BC_CHANNEL_SIZE);
        let (sender_tx, sender_rx) = unbounded_channel();
        let ret = Self {
            tx: sender_tx,
            tx_bc: receiver_tx.clone(),
        };
        let client = ret.clone();
        task::spawn(async move {
            client.receiver(read, receiver_tx)
        });
        let client = ret.clone();
        task::spawn(async move {
            client.sender(write, sender_rx)
        });
        ret
    }

    pub fn monitor(&self) -> Monitor<Resp> {
        self.tx_bc.subscribe()
    }

    pub fn broadcasts(&self) -> UnboundedReceiver<Resp> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                match req {
                    Response::Broadcast(msg) => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    },
                    _ => {}
                }
            }
        });
        rx
    }

    pub fn messages(&self) -> UnboundedReceiver<Resp> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                match req {
                    Response::Broadcast(msg) => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    },
                    Response::Reply { request: _, message } => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });
        rx
    }

    pub fn replies(&self) -> UnboundedReceiver<(Resp, Uuid)> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                match req {
                    Response::Reply { request, message } => {
                        if tx.send((message, request)).is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });
        rx
    }

    pub fn send(&self, msg: Req) -> Option<Uuid> {
        let id = Uuid::new_v4();
        let msg = SenderMsg::Message(
            Request {
                id: id.clone(),
                message: msg
            }
        );
        self.tx.send(msg).ok().map(|_| id)
    }

    pub fn disconnect(self) {
        let _ = self.tx.send(SenderMsg::Drop);
    }

    pub async fn do_query_no_timeout(&self, req: Req) -> Result<Resp, ClientError> {
        let mut replies = self.replies();
        if let Some(id) = self.send(req) {
            while let Some((reply, rx_id)) = replies.recv().await {
                if id == rx_id {
                    return Ok(reply);
                }
            }
            Err(ClientError::ReceiverHungUp)
        } else {
            Err(ClientError::SenderHungUp)
        }
    }

    pub async fn query(&self, req: Req, duration: Duration) -> Result<Resp, ClientError> {
        match timeout(duration, self.do_query_no_timeout(req)).await {
            Ok(x) => x,
            Err(_) => {
                Err(ClientError::Timeout)
            },
        }
    }

    async fn receiver(self, mut read: SplitStream<WsStream>, tx: BcSender<Response<Resp>>) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(msg) => {
                    match msg {
                        tungstenite::Message::Text(text) => {
                            if let Ok(resp) = serde_json::from_str::<Response<Resp>>(&text) {
                                if tx.send(resp).is_err() {
                                    break;
                                }
                            }
                        }
                        tungstenite::Message::Ping(data) => {
                            if self.tx.send(SenderMsg::Pong(data)).is_err() {
                                break;
                            }
                        }
                        tungstenite::Message::Close(_) => {
                            let _ = self.tx.send(SenderMsg::Drop);
                            break;
                        }
                        _ => {}
                    }
                }
                Err(_) => break,
            }
        }
        let _ = self.tx.send(SenderMsg::Drop);
    }

    async fn sender(self, mut write: SplitSink<WsStream, tungstenite::Message>, mut rx: UnboundedReceiver<SenderMsg<Req>>) {
        while let Some(req) = rx.recv().await {
            match req {
                SenderMsg::Drop => {
                    let _ = write.close().await;
                    break;
                }
                SenderMsg::Pong(data) => {
                    let msg = tungstenite::Message::Pong(data);
                    if write.send(msg).await.is_err() {
                        break;
                    }
                }
                SenderMsg::Message(msg) => {
                    let data = serde_json::to_string(&msg).unwrap();
                    let msg = tungstenite::Message::Text(data);
                    if write.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}


