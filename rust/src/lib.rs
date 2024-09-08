//! `broadcast_wsrpc` is simple json-rpc-like library with additional "broadcasting" feature emulating a message bus.
//! The server supports both HTTP (request-response only) and WebSockets (all communication modes) as transport layer.
//!
//! ## Protocol
//!
//! The protocol emulates a "bus" and creates a protocol enabling "pub-sub"-like communication patterns. All messages sent
//! by the server are sent to all clients. This is referred to as broadcasting.
//!
//! There exist 4 message types on the bus:
//!
//! * Requests: client-to-server requests and server broadcasts.
//! * Replies: replies from the server
//! * Notifications: Broadcasts from the server to all clients
//! * Errors: Report errors that have occurred on the bus, such as invalid messages.
//!
//! Clients may only send *request*-messages. Each *request* message contains a unique UUID. Clients are responsible for
//! generating such a unique UUID.
//!
//! Once a client has sent a *request*, the server will broadcast the *request* back to the bus, thereby informing all
//! clients what was requested. The server may answer a request at any later time by broadcasting a *reply* message using
//! the same message id as the request.
//!
//! The server may also broadcast *notification* messages to inform all clients of certain events.
//!
//! Error messages are written to the bus by the server in case parsing a message has failed.
//!
//! ## Websocket Data Type
//!
//! If messages are sent as string frames, the data shall be interpreted as UTF-8 encoded JSON. If messages are sent as
//! binary frames, the data shall be interpreted as being messagepack encoded. Both formats are valid and may be used
//! interchangeably by the implementations. By default, messages are sent as JSON for maximum interoperability with
//! technology stacks such as web-browsers.
//!
//! ## Running a Server
//!
//! ```no_run
//! # use broadcast_wsrpc::server::Server;
//! # use tokio::runtime::Runtime;
//! # use serde::{Serialize, Deserialize};
//! #[derive(Serialize, Deserialize, Clone)]
//! enum Request { Ping }
//!
//! #[derive(Serialize, Deserialize, Clone)]
//! enum Response { Pong }
//!
//! let rt = Runtime::new().unwrap();
//! rt.block_on(async move {
//!     let (mut server, mut rx) = Server::<Request, Response>::new();
//!     server.enable_broadcast_reqrep(true); // requests are broadcasted to all clients
//!     server.listen_ws(&"0.0.0.0:8000".parse().unwrap()).await;
//!     server.listen_http(&"0.0.0.0:8001".parse().unwrap()).await;
//!     while let Some(_request) = rx.recv().await {
//!         // do something with request
//!         // let response = handle(request);
//!         // request.answer(response)
//!     }
//! });
//! ```
//!
//! ## Connecting with a WebSocket Client
//!
//! ```no_run
//! # use tokio::runtime::Runtime;
//! # use serde::{Serialize, Deserialize};
//! # use broadcast_wsrpc::client::{Client, ClientError};
//! # use std::result::Result;
//! # use std::time::Duration;
//! # use url::Url;
//! # #[derive(Serialize, Deserialize, Clone)]
//! # enum Request {
//! #     Ping
//! # }
//! #
//! # #[derive(Serialize, Deserialize, Clone)]
//! # enum Response {
//! #     Pong
//! # }
//! # async fn testfun() -> Result<(), ClientError> {
//! let url = Url::parse("127.0.0.1:8000").unwrap();
//! let client = Client::connect(url, Duration::from_millis(100)).await?;
//! let response: Response = client.request(Request::Ping, Duration::from_millis(100)).await?;
//! # Ok(())
//! # }
//! ```

use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use uuid::Uuid;

pub mod client;
pub mod connection;
mod http;
pub mod server;

/// This trait defines a trait bound that must be implemented for messages
/// used with this crate
pub trait Message: Send + Clone + Serialize {}

impl<T: Send + Clone + Serialize> Message for T {}

pub(crate) struct DebugMessage<'a, T: Message>(&'a T);

impl<'a, T: Message> Debug for DebugMessage<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let serialized = serde_json::to_string(self.0).unwrap();
        f.write_str(&serialized)
    }
}

/// Encodes a message as it sent by clients to the server
#[derive(Clone, Serialize, Deserialize)]
pub struct Request<M: Message> {
    id: Uuid,
    message: M,
    #[serde(skip)]
    sender: Option<Uuid>,
}

#[cfg(test)]
impl<M: Message> Request<M> {
    pub(crate) fn new(msg: M, sender: Option<Uuid>) -> Self {
        Request {
            id: Uuid::new_v4(),
            message: msg,
            sender,
        }
    }
}

/// Encodes a message as it is sent from the server to clients
#[derive(Clone, Serialize, Deserialize)]
pub enum Response<Req: Message, Resp: Message> {
    /// This message type is sent by the server to answer a request
    Reply {
        request: Uuid,
        message: Resp,
        #[serde(skip)]
        sender: Option<Uuid>,
    },
    /// This message type is sent by the server to inform all client about an event
    Notify(Resp),
    /// This message type is sent by the server in case it received an invalid message
    Error(String),
    /// This message type is sent by the server to broadcast requests it received
    /// from a client. This behavior can be customized by adjusting `Server::enable_broadcast_reqrep()`.
    Request { id: Uuid, message: Req },
    /// This message is broadcast if a Request was received but it could not be de-serialized.
    /// In this case, no `Error` message is sent
    InvalidRequest { id: Uuid, description: String },
}
