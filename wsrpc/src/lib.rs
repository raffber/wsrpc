use serde::{Serialize, Deserialize};
use uuid::Uuid;

trait Message : Send + Clone + Serialize + Deserialize {}
impl<T: Send + Clone + Serialize + Deserialize> Message for T {}

struct Request<M: Message> {
    client_id: Uuid,
    request_id: Uuid,
    message: M,
}

type Handler<Req: Message, Resp: Message> = Box<dyn Fn(Request<Req>) -> Option<Resp>>;

struct Server<Req: Message, Resp: Message> {

    handler: Handler<Req, Resp>

}

impl<Req: Message, Resp: Message> Server<Req, Resp> {

}
