# WebSocket & HTTP RPC Library

[![Rust Stable](https://github.com/raffber/wsrpc/actions/workflows/ci.yml/badge.svg)](https://github.com/raffber/wsrpc/actions/workflows/ci.yml)

`wsrpc` is a simple websocket RPC library.
Note that this is not related to [json-rpc](https://en.wikipedia.org/wiki/JSON-RPC) even though it shares some similarities.
This repository provides a rust and python based implementation.

Main transport protocol is WebSockets, but for a pure request-response application an HTTP interface can be provided as well.

The serialization format is based on the `serde_json` to facilitate the interaction between rust applications.

## Protocol

The protocol emulates a "bus" and creates a protocol enabling "pub-sub"-like communication patterns. All messages sent
by the server are sent to all clients. This is referred to as broadcasting.

There exist 4 message types on the bus:

* Requests: client-to-server requests and server broadcasts.
* Replies: replies from the server
* Notifications: Broadcasts from the server to all clients
* Errors: Report errors that have occurred on the bus, such as invalid messages.

Clients may only send *request*-messages. Each *request* message contains a unique UUID. Clients are responsible for
generating such a unique UUID.

Once a client has sent a *request*, the server will broadcast the *request* back to the bus, thereby informing all
clients what was requested. The server may answer a request at any later time by broadcasting a *reply* message using
the same message id as the request.

The server may also broadcast *notification* messages to inform all clients of certain events.

Error messages are written to the bus by the server in case parsing a message has failed.

## Websocket Data Type

If messages are sent as string frames, the data shall be interpreted as UTF-8 encoded JSON. If messages are sent as
binary frames, the data shall be interpreted as being messagepack encoded. Both formats are valid and may be used
interchangeably by the implementations. By default, messages are sent as JSON for maximum interoperability with
technology stacks such as web-browsers.

## Data Model

All serializations follow the [serde data model](https://serde.rs/data-model.html). As mentioned in the previous
section, messages may also be serialized as messagepack and sent as binary websocket frames.

### Requests - Client to Server

```json
{
    "id": "<uuid>",
    "message": {"json encoded": "message"}
}
```

### Requests looped-back from Server to Clients

```json
{
    "Request": {
        "request": "<request-uuid>",
        "message": {"json encoded": "message"}
    }
}
```

### Replies

```json
{
    "Reply": {
        "request": "<request-uuid>",
        "message": {"json encoded": "message"}
    }
}
```

### Notifications

```json
{
    "Notify": {"json encoded": "message"}
}
```

### Errors

```json
{
    "Error": "string describing error"
}
```

## HTTP Server for Request/Response

The request-response part of the protocol may be mapped to a HTTP service. The payload of a HTTP request may be directly
the json encoded request. The server will then generate a *request* message and broadcast it to all client on the
websocket bus. The HTTP response shall contain the data of *reply* message issued by the server.

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license (LICENSE-MIT or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
