#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod client;
mod http;
pub mod server;

pub trait Message: Send + Clone + Serialize {}

impl<T: Send + Clone + Serialize> Message for T {}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request<M: Message> {
    id: Uuid,
    message: M,
    #[serde(skip)]
    sender: Option<Uuid>,
}

impl<M: Message> Request<M> {
    pub fn new(msg: M, sender: Option<Uuid>) -> Self {
        Request {
            id: Uuid::new_v4(),
            message: msg,
            sender,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Response<Req: Message, Resp: Message> {
    Reply {
        request: Uuid,
        message: Resp,
        #[serde(skip)]
        sender: Option<Uuid>,
    },
    Notify(Resp),
    Error(String),
    Request {
        id: Uuid,
        message: Req,
    },
    InvalidRequest {
        id: Uuid,
        description: String,
    },
}
