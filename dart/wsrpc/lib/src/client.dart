import 'dart:async';
import 'dart:convert';
import 'dart:io' show WebSocket;
import 'dart:html' show HttpRequest;
import 'package:async/async.dart' show CancelableOperation;

import 'package:uuid/uuid.dart';

typedef Json = Map<String, dynamic>;

abstract class Rpc {
  Future<Json> request(Json data);
}

class RpcException implements Exception {
  String message;
  RpcException(this.message);
}

class HttpRpc extends Rpc {
  String url;
  final Duration _timeout;

  HttpRpc(this.url, {Duration? timeout})
      : _timeout = timeout ?? Duration(seconds: 1);

  @override
  Future<Json> request(Json data) async {
    final response = await HttpRequest.request(url,
            method: 'GET', sendData: jsonEncode(data), responseType: "json")
        .timeout(_timeout);
    if (response.status != 200) {
      throw RpcException("Request failed with status code ${response.status}");
    }
    if (response.responseText == null) {
      throw RpcException("Response was not a string.");
    }
    return jsonDecode(response.responseText!);
  }
}

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
  Future<Json> request(Json data) async {
    final client = await connect();
    return await client.request(data, _timeout);
  }
}

class Receiver<T> {
  StreamController<T> stream = StreamController();
  Client client;

  Receiver(this.client);

  void close() {
    client.receivers.remove(this);
  }
}

class Client {
  WebSocket ws;
  Set<Receiver> receivers = {};
  late final listenTask =
      CancelableOperation.fromFuture(_rxLoop(ws, receivers));

  Client(this.ws);

  static Future<void> _rxLoop(WebSocket ws, Set<Receiver> receivers) async {
    await for (final msg in ws) {
      for (final rx in receivers) {
        rx.stream.add(msg);
      }
    }
    await ws.close();
  }

  void close() {
    listenTask.cancel();
  }

  void listen(void Function(Receiver) cb) {
    final rx = Receiver(this);
    receivers.add(rx);
    try {
      cb(rx);
    } finally {
      rx.close();
    }
  }

  void sendRequest(Json request, {UuidValue? id}) {
    String strid;
    if (id == null) {
      strid = Uuid().v4();
    } else {
      strid = id.toString();
    }
    final msg = jsonEncode({"id": strid, "message": request});
    ws.add(msg);
  }

  Future<Json> request(Json request, Duration timeout, {UuidValue? id}) async {
    sendRequest(request, id: id);

    return {};
  }
}
