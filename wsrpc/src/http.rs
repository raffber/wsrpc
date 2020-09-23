use std::convert::Infallible;
use std::net::SocketAddr;
use std::string::FromUtf8Error;

use hyper::{Body, Request as HyperRequest, Response as HyperResponse};
use hyper::Server as HyperServer;
use hyper::service::{make_service_fn, service_fn};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::stream::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task;
use uuid::Uuid;

use crate::{Message, Request};
use crate::server::{Requested, SenderMsg, Server};
use crate::server::Reply;

#[derive(Clone)]
pub(crate) struct HttpServer<Req: Message, Resp: Message> {
    server: Server<Req, Resp>,
    tx: UnboundedSender<Requested<Req, Resp>>,
}

impl<Req: Message + 'static + Send + DeserializeOwned, Resp: Message + 'static + Send> HttpServer<Req, Resp> {
    pub(crate) fn spawn(addr: SocketAddr, server: Server<Req, Resp>, tx: UnboundedSender<Requested<Req, Resp>>) -> Self {
        let server = HttpServer { server, tx };
        let ret = server.clone();

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

        let hyper_server = HyperServer::bind(&addr).serve(make_svc);
        log::debug!("HTTP server listening.");
        task::spawn(hyper_server);
        ret
    }

    fn respond_error<T: ToString>(desc: &str, err: T) -> Result<HyperResponse<Body>, hyper::Error> {
        let resp = json!({
                    "error": desc,
                    "reason": err.to_string(),
                });
        let resp = resp.to_string();
        let body: Body = resp.as_bytes().to_vec().into();
        return Ok(HyperResponse::builder()
            .status(400)
            .body(body)
            .unwrap());
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
            },
        };
        let broadcast = Request {
            id: uuid,
            message: request,
        };

        self.server.broadcast_internal(SenderMsg::Request(broadcast)).await;

        // TODO: shutdown server in case this channel breaks
        let _ = self.tx.send(req);
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

async fn collect_body(mut body: Body) -> Result<String, FromUtf8Error> {
    let mut ret = Vec::new();
    while let Some(Ok(x)) = body.next().await {
        ret.extend_from_slice(&x)
    }
    String::from_utf8(ret)
}


