/// Websocket RPC client library in dart
///
/// Currently this library only implements a client.
library broadcast_wsrpc;

export 'src/client.dart'
    show Client, HttpRpc, WsRpc, Rpc, JsonObject, RpcException, InvalidRequest;
