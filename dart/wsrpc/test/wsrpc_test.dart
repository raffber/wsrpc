import 'dart:async';

import 'package:wsrpc/src/client.dart';
import 'package:wsrpc/wsrpc.dart';
import 'dart:io' show Directory, Process, WebSocket;
import 'package:test/test.dart';
import 'package:path/path.dart' show join;

Future<String> getPython() async {
  final result = await Process.run("pipenv", ["--venv"]);
  final venvPath = (result.stdout as String).trim();
  print(venvPath);
  return join(venvPath, "bin", "python");
}

Future<Completer> spawnPythonServer() async {
  final python = await getPython();
  final testServer = join(Directory.current.path, "test", "test_server.py");
  final proc = Process.run(python, [testServer]);
  final ret = Completer();
  ret.complete(proc);
  await Future.delayed(Duration(milliseconds: 100));
  return ret;
}

void main() {
  group('Basics', () {
    test('Check Request', () async {
      final proc = await spawnPythonServer();
      final ws = await WebSocket.connect("ws://127.0.0.1:7479");
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
      final ws = await WebSocket.connect("ws://127.0.0.1:7479");
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
  });
}
