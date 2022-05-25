use std::io;
use std::time::{Duration, Instant};

use async_tungstenite::tokio::{connect_async, TokioAdapter};
use async_tungstenite::tungstenite::Message as WsMessage;
use async_tungstenite::WebSocketStream;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio::io::ErrorKind;
use tokio::net::TcpStream;
use tokio::sync::broadcast::{channel as bc_channel, Receiver as BcReceiver, Sender as BcSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task;
use tokio::time::sleep;
use tokio::time::timeout;
use url::Url;
use uuid::Uuid;

use crate::{Message, Request, Response};

type WsStream = WebSocketStream<TokioAdapter<TcpStream>>;

const BC_CHANNEL_SIZE: usize = 1000;

pub type Monitor<Req, Resp> = BcReceiver<Response<Req, Resp>>;

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
    tx_bc: BcSender<Response<Req, Resp>>,
}

impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message + DeserializeOwned>
Client<Req, Resp>
{
    pub async fn connect<A>(url: A, duration: Duration) -> io::Result<Self>
        where
            A: Into<Url>,
    {
        let start = Instant::now();
        let url = url.into();

        loop {
            match connect_async(url.clone()).await {
                Ok((stream, _)) => {
                    return Ok(Self::with_stream(stream));
                }
                // TODO: filter by fatal errors
                // Err(WsError::Io(x)) => {
                //     // this is fatal
                //     return Err(x);
                // }
                _ => {}
            }
            if start.elapsed().as_secs_f32() > duration.as_secs_f32() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        Err(io::Error::new(
            ErrorKind::TimedOut,
            "Timeout during connection attempts.",
        ))
    }

    pub fn with_stream(stream: WsStream) -> Self {
        let _ = stream.get_ref().get_ref().set_nodelay(true);
        let (write, read) = stream.split();
        let (receiver_tx, _) = bc_channel(BC_CHANNEL_SIZE);
        let (sender_tx, sender_rx) = unbounded_channel();
        let ret = Self {
            tx: sender_tx,
            tx_bc: receiver_tx.clone(),
        };
        let client = ret.clone();
        task::spawn(async move { client.receiver(read, receiver_tx).await });
        let client = ret.clone();
        task::spawn(async move { client.sender(write, sender_rx).await });
        ret
    }

    pub fn monitor(&self) -> Monitor<Req, Resp> {
        self.tx_bc.subscribe()
    }

    pub fn notifications(&self) -> UnboundedReceiver<Resp> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                match req {
                    Response::Notify(msg) => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    }
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
                    Response::Notify(msg) => {
                        if tx.send(msg).is_err() {
                            break;
                        }
                    }
                    Response::Reply {
                        request: _,
                        message,
                        sender: _,
                    } => {
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
                    Response::Reply {
                        request, message, ..
                    } => {
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
        let msg = Request {
            id: id.clone(),
            message: msg,
            sender: None,
        };
        let msg = SenderMsg::Message(msg);
        self.tx.send(msg).ok().map(|_| id)
    }

    pub fn disconnect(self) {
        log::debug!("Disconnecting sender");
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
            log::error!("Receiver hung up!");
            Err(ClientError::ReceiverHungUp)
        } else {
            log::error!("Sender hung up!");
            Err(ClientError::SenderHungUp)
        }
    }

    pub async fn query(&self, req: Req, duration: Duration) -> Result<Resp, ClientError> {
        match timeout(duration, self.do_query_no_timeout(req)).await {
            Ok(x) => x,
            Err(_) => Err(ClientError::Timeout),
        }
    }

    async fn receiver(self, mut read: SplitStream<WsStream>, tx: BcSender<Response<Req, Resp>>) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(msg) => match msg {
                    WsMessage::Text(text) => {
                        if let Ok(resp) = serde_json::from_str::<Response<Req, Resp>>(&text) {
                            log::debug!("Received: {}", text);
                            let _ = tx.send(resp).is_err();
                        }
                    }
                    WsMessage::Ping(data) => {
                        let _ = self.tx.send(SenderMsg::Pong(data)).is_err();
                    }
                    WsMessage::Close(_) => {
                        let _ = self.tx.send(SenderMsg::Drop);
                        log::debug!("Closing websocket connection by remote");
                        break;
                    }
                    _ => {}
                },
                Err(x) => {
                    log::error!("Websocket error occurred: {}", x);
                    break;
                }
            }
        }
        log::debug!("Receiver quitting. Dropping sender.");
        let _ = self.tx.send(SenderMsg::Drop);
    }

    async fn sender(
        self,
        mut write: SplitSink<WsStream, WsMessage>,
        mut rx: UnboundedReceiver<SenderMsg<Req>>,
    ) {
        while let Some(req) = rx.recv().await {
            match req {
                SenderMsg::Drop => {
                    let _ = write.close().await;
                    log::debug!("Sender received drop. Quitting.");
                    break;
                }
                SenderMsg::Pong(data) => {
                    let msg = WsMessage::Pong(data);
                    if write.send(msg).await.is_err() {
                        break;
                    }
                }
                SenderMsg::Message(msg) => {
                    let data = serde_json::to_string(&msg).unwrap();
                    log::debug!("Sending: {}", data);
                    let msg = WsMessage::Text(data);
                    if write.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        }
        log::debug!("Sender quitting.")
    }
}
