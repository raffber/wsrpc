import 'dart:async';
import 'dart:convert';
import 'dart:io' show ContentType, HttpClient, WebSocket;

import 'package:uuid/uuid.dart';

typedef JsonObject = Map<String, dynamic>;

abstract class Rpc {
  Future<JsonObject> request(JsonObject data);
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
  Future<JsonObject> request(JsonObject data) async {
    final client = HttpClient();
    final request = await client.postUrl(Uri.parse(url));
    request.headers.contentType =
        ContentType('application', 'json', charset: 'utf-8');
    request.write(jsonEncode(data));
    final response = await request.done.timeout(_timeout);

    if (response.statusCode != 200) {
      throw RpcException(
          "Request failed with status code ${response.statusCode}");
    }
    final stringData = await response.transform(utf8.decoder).join();
    return jsonDecode(stringData);
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
  Future<JsonObject> request(JsonObject data) async {
    final client = await connect();
    return await client.request(data, _timeout);
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

class Client {
  WebSocket ws;
  Set<Receiver<JsonObject>> receivers = {};
  late final listenTask = Completer();

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

  Future<void> close() async {
    await ws.close();
    await listenTask.future;
  }

  Future<S> listen<S>(Future<S> Function(Receiver<JsonObject>) cb) async {
    final rx = Receiver<JsonObject>(this);
    receivers.add(rx);
    try {
      return await cb(rx);
    } finally {
      rx.close();
    }
  }

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

  Stream<JsonObject> notifications() {
    if (ws.readyState != WebSocket.open) {
      throw StateError("Client has already been closed.");
    }
    final rx = Receiver<JsonObject>(this);
    receivers.add(rx);
    return rx.stream
        .where((event) => event.containsKey("Notify"))
        .map((event) => event["Notify"]);
  }

  Stream<JsonObject> replies() {
    if (ws.readyState != WebSocket.open) {
      throw StateError("Client has already been closed.");
    }
    final rx = Receiver<JsonObject>(this);
    receivers.add(rx);
    return rx.stream
        .where((event) => event.containsKey("Reply"))
        .map((event) => event["Reply"]);
  }

  Future<JsonObject> request(JsonObject request, Duration timeout,
      {UuidValue? id}) async {
    if (ws.readyState != WebSocket.open) {
      throw StateError("Client has already been closed.");
    }
    return await listen((rx) async {
      final id = Uuid().v4obj();
      sendRequest(request, id: id);
      final strid = id.toString();
      await for (final msg in rx.stream) {
        if (!msg.containsKey("Reply")) {
          continue;
        }
        var response = msg["Reply"] as JsonObject;
        if (response["request"] == strid) {
          return response["message"];
        }
        break;
      }
      return {};
    }).timeout(timeout);
  }
}
