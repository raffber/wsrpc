import 'dart:async';
import 'dart:convert';
import 'dart:io' show ContentType, HttpClient, WebSocket;

import 'package:uuid/uuid.dart';

typedef JsonObject = Map<String, dynamic>;

/// Abstract interface to perform RPC request-response calls
abstract class Rpc {
  /// Perform a request and wait for the response. If `timeout` is not
  /// given a default timeout shall be applied.
  Future<JsonObject> request(JsonObject data, {Duration? timeout});

  /// Close the connection to the remote server, if applicable
  Future<void> close() async {}
}

/// Exception thrown in case an exception occurs on the RPC layer
class RpcException implements Exception {
  String message;
  RpcException(this.message);
}

/// Exception thrown in case an invalid request was sent to the bus
/// and the server rejected it
class InvalidRequest implements Exception {
  String message;
  InvalidRequest(this.message);
}

/// RPC interface implementation using HTTP to connect to the server
class HttpRpc extends Rpc {
  String url;
  final Duration _timeout;

  HttpRpc(this.url, {Duration? timeout})
      : _timeout = timeout ?? Duration(seconds: 1);

  @override
  Future<JsonObject> request(JsonObject data, {Duration? timeout}) async {
    final client = HttpClient();
    final request = await client.postUrl(Uri.parse(url));
    request.headers.contentType =
        ContentType('application', 'json', charset: 'utf-8');
    request.write(jsonEncode(data));
    final response = await request.close().timeout(timeout ?? _timeout);

    if (response.statusCode != 200) {
      throw RpcException(
          "Request failed with status code ${response.statusCode}");
    }
    final stringData = await response.transform(utf8.decoder).join();
    return jsonDecode(stringData);
  }
}

/// RPC interface implementation using websockets to connect to the server
class WsRpc extends Rpc {
  String url;
  Client? _client;
  final Duration _timeout;

  WsRpc(this.url, {Duration? timeout})
      : _timeout = timeout ?? Duration(seconds: 1);

  Future<Client> connect() async {
    if (_client != null) {
      return _client!;
    }
    _client = Client(await WebSocket.connect(url));
    return _client!;
  }

  @override
  Future<JsonObject> request(JsonObject data, {Duration? timeout}) async {
    final client = await connect();
    return await client.request(data, timeout ?? _timeout);
  }

  @override
  Future<void> close() async {
    if (_client != null) {
      await _client!.close();
    }
  }
}

class Receiver<T> {
  final _stream = StreamController<T>();
  Client client;

  Receiver(this.client);

  Stream<T> get stream => _stream.stream;

  void close() {
    client.receivers.remove(this);
  }
}

/// A websocket RPC client to connect to a websocket wsrpc server.
///
/// To create an instance of this class, an already connected `WebSocket`
/// instance needs to be passed.
///
/// Make sure to close the websocket connection by calling `Client.close()`.
class Client {
  WebSocket ws;
  Set<Receiver<JsonObject>> receivers = {};
  late final listenTask = Completer();

  /// Create a new `Client` instance with an **already** connected `WebSocket`.
  /// This will automatically spawn a receiver loop to listen to messages
  /// on the websocket connection.
  Client(this.ws) {
    listenTask.complete(_rxLoop(ws, receivers));
  }

  static Future<void> _rxLoop(WebSocket ws, Set<Receiver> receivers) async {
    await for (final msg in ws) {
      if (msg is String) {
        try {
          final parsedMsg = jsonDecode(msg);
          for (final rx in receivers) {
            rx._stream.add(parsedMsg);
          }
        } on FormatException {
          continue;
        }
      }
    }
  }

  /// Close the underlying websocket connection. Afterwards, this client
  /// cannot be used anymore.
  Future<void> close() async {
    await ws.close();
    await listenTask.future;
  }

  /// Registers a receiver and start listen to messages on the message bus.
  /// When the internal callback returns, the receiver is automatically unregistered.
  ///
  ///   await client.listen((stream) async {
  ///     // do something with `stream`
  ///     async for (final message in stream) {
  ///       // do something with `message`
  ///     }
  ///   })
  Future<S> listen<S>(Future<S> Function(Stream<JsonObject>) cb) async {
    final rx = Receiver<JsonObject>(this);
    receivers.add(rx);
    try {
      return await cb(rx._stream.stream);
    } finally {
      rx.close();
    }
  }

  /// Registers a receiver and start listen to messages on the message bus.
  /// The callback returns a stream of messages based on the received messages.
  /// Once the stream is done, the receiver is automatically unregistrered.
  Stream<S> stream<S>(Stream<S> Function(Stream<JsonObject>) cb) async* {
    final rx = Receiver<JsonObject>(this);
    receivers.add(rx);
    try {
      await for (final message in cb(rx._stream.stream)) {
        yield message;
      }
    } finally {
      rx.close();
    }
  }

  /// Send a request message to the bus, optionally a request-id can
  /// be provided, such that corresponding responses could be tracked.
  void sendRequest(JsonObject request, {UuidValue? id}) {
    if (ws.readyState != WebSocket.open) {
      throw StateError("Client has already been closed.");
    }
    String strid;
    if (id == null) {
      strid = Uuid().v4();
    } else {
      strid = id.toString();
    }
    final msg = jsonEncode({"id": strid, "message": request});
    ws.add(msg);
  }

  /// Send a request to the bus and wait for an answer up the given timeout.
  /// Throws an `InvalidRequest` in case the server rejected the request
  Future<JsonObject> request(JsonObject request, Duration timeout,
      {UuidValue? id}) async {
    if (ws.readyState != WebSocket.open) {
      throw StateError("Client has already been closed.");
    }
    return await listen((rx) async {
      final id = Uuid().v4obj();
      sendRequest(request, id: id);
      final strid = id.toString();
      await for (final msg in rx) {
        if (msg.containsKey("Reply")) {
          var response = msg["Reply"] as JsonObject;
          if (response["request"] == strid) {
            return response["message"];
          }
        } else if (msg.containsKey("InvalidRequest")) {
          var response = msg["InvalidRequest"] as JsonObject;
          if (response["id"] == strid) {
            final description = response["description"] as String;
            final req = jsonEncode(request);
            throw InvalidRequest(
                "Server reject request: $description. Request was: $req");
          }
        }
      }
    }).timeout(timeout);
  }
}
