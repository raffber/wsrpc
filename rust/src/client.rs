//! This module implements a WebSocket client to connect to a message bus.
//!
//! Refer to the [`crate::client::Client`] documentation for more information.

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
use tokio::sync::broadcast::{channel as bc_channel, Sender as BcSender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task;
use tokio::time::sleep;
use url::Url;
use uuid::Uuid;

use crate::{Message, Request, Response};

type WsStream = WebSocketStream<TokioAdapter<TcpStream>>;

const BC_CHANNEL_SIZE: usize = 1000;

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

impl From<io::Error> for ClientError {
    fn from(x: io::Error) -> Self {
        ClientError::Io(x)
    }
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

///
/// A weboscket client to connect to the message bus.
/// Use as follows:
///
/// ```no_run
/// # use tokio::runtime::Runtime;
/// # use serde::{Serialize, Deserialize};
/// # use broadcast_wsrpc::client::{Client, ClientError};
/// # use std::result::Result;
/// # use std::time::Duration;
/// # use url::Url;
/// # #[derive(Serialize, Deserialize, Clone)]
/// # enum Request {
/// #     Ping
/// # }
/// #
/// # #[derive(Serialize, Deserialize, Clone)]
/// # enum Response {
/// #     Pong
/// # }
/// # async fn testfun() -> Result<(), ClientError> {
/// let url = Url::parse("127.0.0.1:8000").unwrap();
/// let client = Client::connect(url, Duration::from_millis(100)).await?;
/// let response: Response = client.request(Request::Ping, Duration::from_millis(100)).await?;
/// # Ok(())
/// # }
/// ```
///
/// The client can be cloned but it re-uses the same underlying websocket connection.
/// The connection closes once the `disconnect()` function is called or the last client is dropped.
impl<Req: 'static + Message + DeserializeOwned, Resp: 'static + Message + DeserializeOwned>
    Client<Req, Resp>
{
    /// Attempt to connect to the websocket server for the give timeout. Multiple connection attempts
    /// are made until `timeout` time as expired
    pub async fn connect<A>(url: A, timeout: Duration) -> io::Result<Self>
    where
        A: Into<Url>,
    {
        let start = Instant::now();
        let url = url.into();

        loop {
            if let Ok((stream, _)) = connect_async(url.clone()).await {
                return Ok(Self::with_stream(stream));
            }
            if start.elapsed().as_secs_f32() > timeout.as_secs_f32() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        Err(io::Error::new(
            ErrorKind::TimedOut,
            "Timeout during connection attempts.",
        ))
    }

    /// Initialize a client with a websocket stream that was already connected
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

    /// Subscribe to monitor all messages on the message bus
    pub fn monitor(&self) -> UnboundedReceiver<Response<Req, Resp>> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                if tx.send(req).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// Subscribe to all [`crate::Response::Notify`] messages on the message bus
    pub fn notifications(&self) -> UnboundedReceiver<Resp> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                if let Response::Notify(msg) = req {
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            }
        });
        rx
    }

    /// Subscribe to all replies and notifications
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

    /// Subscribe to all replies sent to the bus
    pub fn replies(&self) -> UnboundedReceiver<(Resp, Uuid)> {
        let mut rx_bc = self.tx_bc.subscribe();
        let (tx, rx) = unbounded_channel();
        task::spawn(async move {
            while let Ok(req) = rx_bc.recv().await {
                if let Response::Reply {
                    request, message, ..
                } = req
                {
                    if tx.send((message, request)).is_err() {
                        break;
                    }
                }
            }
        });
        rx
    }

    /// Send a message to the bus
    pub fn send(&self, msg: Req) -> Option<Uuid> {
        let id = Uuid::new_v4();
        let msg = Request {
            id,
            message: msg,
            sender: None,
        };
        let msg = SenderMsg::Message(msg);
        self.tx.send(msg).ok().map(|_| id)
    }

    /// Disconnect the client
    pub fn disconnect(self) {
        log::debug!("Disconnecting sender");
        let _ = self.tx.send(SenderMsg::Drop);
    }

    /// Send a request and wait for the response without timeout
    pub async fn request_no_timeout(&self, request: Req) -> Result<Resp, ClientError> {
        let mut replies = self.replies();
        if let Some(id) = self.send(request) {
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

    /// Send a request and wait for the response up to the given timeout
    pub async fn request(&self, request: Req, timeout: Duration) -> Result<Resp, ClientError> {
        match tokio::time::timeout(timeout, self.request_no_timeout(request)).await {
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
