#![allow(dead_code)]

use serde::{Serialize, Deserialize};
use uuid::Uuid;

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
pub enum Response<M: Message> {
    Reply {
        request: Uuid,
        message: M,
    },
    Notify(M),
    Error(String)
}

