/// Websocket RPC client library in dart
///
/// Currently this library only implements a client.
library wsrpc;

export 'src/client.dart'
    show Client, HttpRpc, WsRpc, Rpc, JsonObject, RpcException;
