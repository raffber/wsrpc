use std::net::SocketAddr;

use tokio::sync::mpsc::UnboundedSender;

use crate::Message;
use crate::server::{Requested, Server};
use hyper::Server as HyperServer;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request as HyperRequest, Response as HyperResponse, StatusCode};
use std::convert::Infallible;
use tokio::task;

#[derive(Clone)]
pub(crate) struct HttpServer<Req: Message, Resp: Message> {
    server: Server<Req, Resp>,
    tx: UnboundedSender<Requested<Req, Resp>>,
}

impl<Req: Message + 'static + Send, Resp: Message + 'static + Send> HttpServer<Req, Resp> {
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

    async fn handle(self, req: HyperRequest<Body>) -> Result<HyperResponse<Body>, hyper::Error> {
        todo!()
    }
}


