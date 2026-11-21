"""Real Unix/WebSocket transport, deterministic synthetic target. No native client."""
import base64
import hashlib
import json
import socketserver
import struct
import threading
import uuid
from pathlib import Path
from handoff_receipts.common import append, now, canonical


class FakeDaemon:
    def __init__(self, root, socket):
        self.root, self.socket = Path(root), Path(socket)
        self.root.mkdir(parents=True, exist_ok=True)
        self.threads, self.calls, self.connections = {}, [], set()
        self.lock = threading.RLock()
        self.next_status = 'completed'
        self.unmaterialized = 0
        self.inject_context = True
        self.sensitive_roles = set()
        self.empty_preflight = False
        self.drop_start_response = False
        self.defer_rollout = False
        self.hide_rollout_path = False
        owner = self

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                ident = uuid.uuid4().hex
                owner.connections.add(ident)
                try:
                    header = b''
                    while not header.endswith(b'\r\n\r\n'):
                        chunk = self.rfile.read(1)
                        if not chunk:
                            return
                        header += chunk
                    key = next(line.split(b':', 1)[1].strip() for line in header.split(b'\r\n') if line.lower().startswith(b'sec-websocket-key:'))
                    accept = base64.b64encode(hashlib.sha1(key + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
                    self.wfile.write(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
                    while True:
                        head = self.rfile.read(2)
                        if len(head) != 2 or head[0] & 15 == 8:
                            return
                        n = head[1] & 127
                        if n == 126:
                            n = struct.unpack('!H', self.rfile.read(2))[0]
                        if n == 127:
                            n = struct.unpack('!Q', self.rfile.read(8))[0]
                        mask = self.rfile.read(4) if head[1] & 128 else None
                        raw = self.rfile.read(n)
                        if mask:
                            raw = bytes(v ^ mask[i % 4] for i, v in enumerate(raw))
                        request = json.loads(raw)
                        with owner.lock:
                            owner.calls.append({'connection': ident, **request})
                            response = owner.dispatch(request)
                        if request['method'] == 'turn/start' and owner.drop_start_response:
                            return
                        if 'id' not in request:
                            continue
                        data = canonical({'id': request['id'], **response}).encode()
                        prefix = b'\x81' + (bytes([len(data)]) if len(data) < 126 else b'\x7e' + struct.pack('!H', len(data)))
                        self.wfile.write(prefix + data)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                finally:
                    owner.connections.discard(ident)

        class Server(socketserver.ThreadingUnixStreamServer):
            daemon_threads = True
        self.server = Server(str(self.socket), Handler)
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)

    def dispatch(self, request):
        method, p = request['method'], request.get('params', {})
        if method in ('initialize', 'initialized'):
            return {'result': {}}
        if method == 'thread/start':
            tid = str(uuid.uuid4())
            path = self.root / f'rollout-{tid}.jsonl'
            if self.defer_rollout:
                pass
            elif self.empty_preflight:
                path.touch()
            else:
                append(path, {'type': 'session_meta', 'payload': {'id': tid, 'cwd': p['cwd'],
                             'account_id': 'SYNTHETIC_ACCOUNT_SENTINEL'}})
            self.threads[tid] = {'id': tid, 'cwd': p['cwd'], 'path': str(path), 'status': {'type': 'idle'}, 'turns': []}
            return {'result': {'thread': {**self.threads[tid], 'path': None} if self.hide_rollout_path else self.threads[tid]}}
        thread = self.threads[p['threadId']]
        if method == 'thread/read':
            return {'result': {'thread': {**thread, 'path': None} if self.hide_rollout_path and not thread['turns'] else thread}}
        if method == 'thread/turns/list':
            if self.defer_rollout and not thread['turns']:
                return {'error': {'code': -32000, 'message': 'thread not materialized'}}
            if self.unmaterialized:
                self.unmaterialized -= 1
                return {'error': {'code': -32000, 'message': 'thread not materialized'}}
            return {'result': {'data': thread['turns'], 'nextCursor': None}}
        if method == 'turn/start':
            turn = {'id': str(uuid.uuid4()), 'status': self.next_status}
            body = p['input'][0]['text']
            def event(kind, **values):
                append(thread['path'], {'timestamp': now(), 'type': 'event_msg',
                                       'payload': {'type': kind, 'turn_id': turn['id'], **values}})
            if not Path(thread['path']).exists() or not Path(thread['path']).stat().st_size:
                append(thread['path'], {'type': 'session_meta', 'payload': {'id': thread['id'], 'account_id': 'SYNTHETIC_ACCOUNT_SENTINEL'}})
            event('task_started', **({'private': 'private-start@example.invalid'} if 'start' in self.sensitive_roles else {}))
            if self.inject_context and not thread['turns']:
                append(thread['path'], {'type': 'response_item', 'payload': {'type': 'message', 'role': 'user',
                       'content': [{'type': 'input_text', 'text': 'Injected AGENTS: PRIVATE_CONTEXT_SENTINEL context@example.invalid'}]}})
                append(thread['path'], {'type': 'world_state', 'payload': {'private': 'PRIVATE_WORLD_SENTINEL world@example.invalid'}})
                append(thread['path'], {'type': 'turn_context', 'payload': {'note': 'UNNEEDED_CONTEXT_SENTINEL'}})
            append(thread['path'], {'timestamp': now(), 'type': 'response_item', 'payload': {
                'type': 'message', 'role': 'user', 'content': [{'type': 'input_text', 'text': body}],
                **({'private': 'private-input@example.invalid'} if 'input' in self.sensitive_roles else {})}})
            append(thread['path'], {'type': 'response_item', 'payload': {'type': 'message', 'role': 'assistant',
                   'phase': 'final_answer', 'content': [{'type': 'output_text', 'text':
                   'private-final@example.invalid' if 'final_output' in self.sensitive_roles else 'HR1-ACK'}]}})
            if self.next_status == 'completed':
                event('task_complete', last_agent_message='private-final@example.invalid' if 'final_output' in self.sensitive_roles else 'HR1-ACK',
                      **({'private': 'private-terminal@example.invalid'} if 'terminal' in self.sensitive_roles else {}))
            elif self.next_status == 'failed':
                event('task_failed')
            elif self.next_status == 'interrupted':
                event('turn_aborted')
            thread['turns'].append(turn)
            return {'result': {'turn': turn}}
        return {'error': {'code': -32601, 'message': 'unsupported mock method'}}

    def __enter__(self):
        self.worker.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.socket.unlink(missing_ok=True)
