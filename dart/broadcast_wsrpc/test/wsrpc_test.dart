import 'dart:async';

import 'package:broadcast_wsrpc/lib.dart';
import 'dart:io'
    show Directory, HttpClient, Process, SocketException, WebSocket;
import 'package:test/test.dart';
import 'package:path/path.dart' show join;

Future<String> getPython() async {
  final result = await Process.run("pipenv", ["--venv"]);
  final venvPath = (result.stdout as String).trim();
  return join(venvPath, "bin", "python");
}

Future<Completer> spawnPythonServer() async {
  final python = await getPython();
  final testServer = join(Directory.current.path, "test", "test_server.py");
  final proc = Process.run(python, [testServer]);
  final ret = Completer();
  ret.complete(proc);
  return ret;
}

Future<Completer> spawnHttpPythonServer() async {
  final python = await getPython();
  final testServer =
      join(Directory.current.path, "test", "test_http_server.py");
  final proc = Process.run(python, [testServer]);
  final ret = Completer();
  ret.complete(proc);
  return ret;
}

Future<WebSocket> connect(String url) async {
  while (true) {
    try {
      return await WebSocket.connect(url);
    } on SocketException catch (_) {
      await Future.delayed(Duration(milliseconds: 10));
    }
  }
}

Future<void> connectHttp(String url) async {
  while (true) {
    try {
      final client = HttpClient();
      await client.postUrl(Uri.parse(url));
      return;
    } on SocketException catch (_) {
      await Future.delayed(Duration(milliseconds: 10));
    }
  }
}

void main() {
  group('Basics', () {
    test('Check Request', () async {
      final proc = await spawnPythonServer();
      final ws =
          await connect("ws://127.0.0.1:7479").timeout(Duration(seconds: 2));
      final client = Client(ws);
      final reply =
          await client.request({'Foo': 'Bar'}, Duration(milliseconds: 300));
      assert(reply['Foo'] == 'Bar');

      client.sendRequest({'Quit': null});
      await client.close();
      await proc.future;
    });

    test('Invalid Request', () async {
      final proc = await spawnPythonServer();
      final ws =
          await connect("ws://127.0.0.1:7479").timeout(Duration(seconds: 2));
      final client = Client(ws);
      var invalidRequest = false;
      try {
        await client
            .request({'Something': 'Strange'}, Duration(milliseconds: 300));
      } on InvalidRequest {
        invalidRequest = true;
      }
      assert(invalidRequest);

      client.sendRequest({'Quit': null});
      await client.close();
      await proc.future;
    });

    test('http', () async {
      final proc = await spawnHttpPythonServer();
      final url = "http://127.0.0.1:7480";
      await connectHttp(url).timeout(Duration(seconds: 2));
      final rpc = HttpRpc(url);
      final reply = await rpc.request({"Foo": "Bar"});
      assert(reply['Foo'] == 'Bar');
      try {
        await rpc.request({'Shutdown': null});
        // ignore: empty_catches
      } catch (err) {}
      await proc.future;
    });
  });
}
