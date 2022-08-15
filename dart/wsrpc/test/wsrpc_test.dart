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

void main() {
  group('A group of tests', () {
    test('Check ping', () async {
      final python = await getPython();
      final testServer = join(Directory.current.path, "test", "test_server.py");
      final proc = Process.run(python, [testServer]);
      await Future.delayed(Duration(milliseconds: 100));
      final ws = await WebSocket.connect("ws://127.0.0.1:7479");
      final client = Client(ws);
      final reply =
          await client.request({'Foo': 'Bar'}, Duration(milliseconds: 300));
      assert(reply['Foo'] == 'Bar');

      client.sendRequest({'Quit': null});
      await client.close();
      await proc;
    });
  });
}
