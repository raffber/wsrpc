use std::convert::Infallible;
use std::net::SocketAddr;

use futures::FutureExt;
use hyper::service::{make_service_fn, service_fn};
use hyper::Server as HyperServer;
use hyper::{Body, Request as HyperRequest, Response as HyperResponse};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::oneshot;
use tokio::task;
use uuid::Uuid;

use crate::server::Reply;
use crate::server::{Requested, SenderMsg, Server};
use crate::{Message, Response};

#[derive(Clone)]
pub(crate) struct HttpServer<Req: Message, Resp: Message> {
    server: Server<Req, Resp>,
}

impl<Req: Message + 'static + Send + DeserializeOwned, Resp: Message + 'static + Send>
    HttpServer<Req, Resp>
{
    pub(crate) fn spawn<T: Into<SocketAddr>>(
        addr: T,
        server: Server<Req, Resp>,
    ) -> oneshot::Sender<()> {
        let server = HttpServer { server };

        let make_svc = make_service_fn(move |_| {
            let server = server.clone();
            async move {
                let server = server.clone();
                Ok::<_, Infallible>(service_fn(move |req| {
                    let server = server.clone();
                    server.handle(req)
                }))
            }
        });

        let (cancel_tx, cancel_rx) = oneshot::channel();

        let hyper_server = HyperServer::bind(&addr.into()).serve(make_svc);
        let graceful =
            hyper_server.with_graceful_shutdown(async move { cancel_rx.map(|_| ()).await });

        log::debug!("HTTP server listening.");
        task::spawn(graceful);
        cancel_tx
    }

    fn respond_error<T: ToString>(desc: &str, err: T) -> Result<HyperResponse<Body>, hyper::Error> {
        let resp = json!({
            "error": desc,
            "reason": err.to_string(),
        });
        let resp = resp.to_string();
        let body: Body = resp.as_bytes().to_vec().into();
        return Ok(HyperResponse::builder().status(400).body(body).unwrap());
    }

    async fn handle(self, req: HyperRequest<Body>) -> Result<HyperResponse<Body>, hyper::Error> {
        let data = hyper::body::to_bytes(req.into_body()).await?;
        let data = match String::from_utf8(data.to_vec()) {
            Ok(data) => data,
            Err(err) => {
                return Self::respond_error("Cannot decode body as UTF-8.", err);
            }
        };
        let request: Req = match serde_json::from_str(&data) {
            Ok(x) => x,
            Err(err) => {
                return Self::respond_error("Cannot deserialize body", err);
            }
        };
        let (tx, rx) = oneshot::channel();
        let uuid = Uuid::new_v4();
        let req = Requested {
            msg: request.clone(),
            reply: Reply {
                id: uuid,
                server: self.server.clone(),
                direct: Some(tx),
                sender: None
            },
        };
        let broadcast = Response::Request {
            id: uuid,
            message: request,
        };

        self.server.broadcast_internal(SenderMsg::Message(broadcast));

        // TODO: shutdown server in case this channel breaks
        let _ = self.server.dispatch_request(req).await;
        let resp = if let Ok(x) = rx.await {
            x
        } else {
            return Self::respond_error("Cannot process request", "Unknown error");
        };
        let ret = serde_json::to_vec(&resp).unwrap();
        let body: Body = ret.into();
        Ok(HyperResponse::new(body))
    }
}
