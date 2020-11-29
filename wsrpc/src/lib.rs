#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod http;
pub mod client;
pub mod server;

pub trait Message: Send + Clone + Serialize {}

impl<T: Send + Clone + Serialize> Message for T {}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request<M: Message> {
    id: Uuid,
    message: M,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Response<Req: Message, Resp: Message> {
    Reply {
        request: Uuid,
        message: Resp,
    },
    Notify(Resp),
    Error(String),
    Request {
        id: Uuid,
        message: Req,
    },
}

