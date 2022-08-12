import 'dart:convert';
import 'dart:io';

import 'package:uuid/uuid.dart';

typedef Json = Map<String, dynamic>;

abstract class Rpc {
  Future<Json> request(Json data);
}

class HttpRpc extends Rpc {
  @override
  Future<Json> request(Json data) async {
    // TODO: implement request
    throw UnimplementedError();
  }
}

class WsRpc extends Rpc {
  @override
  Future<Json> request(Json data) async {
    // TODO: implement request
    throw UnimplementedError();
  }
}

class Client {
  WebSocket ws;

  Client(this.ws);

  void sendRequest(Json request, UuidValue? id) async {
    String strid;
    if (id == null) {
      strid = Uuid().v4();
    } else {
      strid = id.toString();
    }
    final msg = jsonEncode({"id": strid, "message": request});
    ws.add(msg);
  }
}
