"""Local Unix WebSocket JSON-RPC transport, retained from the capture prototype."""
import base64, hashlib, json, os, pathlib, socket, struct, time

class Rpc:
    def __init__(self, path, name='handoff_receipts'):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(30)
        self.sock.connect(str(pathlib.Path(path).resolve(strict=True)))
        self.buffer = b''
        self.counter = 0
        self.notifications = []
        key = base64.b64encode(os.urandom(16)).decode()
        request = ('GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n'
                   'Connection: Upgrade\r\nSec-WebSocket-Key: '+key+'\r\n'
                   'Sec-WebSocket-Version: 13\r\n\r\n')
        self.sock.sendall(request.encode())
        while b'\r\n\r\n' not in self.buffer:
            chunk = self.sock.recv(4096)
            if not chunk: raise RuntimeError('Socket closed during HTTP upgrade')
            self.buffer += chunk
            if len(self.buffer)>65536: raise RuntimeError('Oversized HTTP response')
        header,self.buffer = self.buffer.split(b'\r\n\r\n',1)
        if b' 101 ' not in header.split(b'\r\n')[0]: raise RuntimeError('WebSocket upgrade rejected')
        headers = dict(line.split(b':',1) for line in header.split(b'\r\n')[1:] if b':' in line)
        headers = {k.lower():v.strip() for k,v in headers.items()}
        expected=base64.b64encode(hashlib.sha1((key+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest())
        if headers.get(b'sec-websocket-accept') != expected: raise RuntimeError('Invalid WebSocket accept')
        self.call('initialize',{'clientInfo':{'name':name,'version':'1.0'},'capabilities':{'experimentalApi':True}})
        self.send({'method':'initialized','params':{}})

    def exact(self,n):
        while len(self.buffer)<n:
            chunk=self.sock.recv(max(4096,n-len(self.buffer)))
            if not chunk:raise RuntimeError('Socket closed')
            self.buffer+=chunk
        data,self.buffer=self.buffer[:n],self.buffer[n:]
        return data

    def frame(self,data,opcode=1):
        mask=os.urandom(4);n=len(data)
        prefix=bytes([0x80|opcode])
        if n<126:prefix+=bytes([0x80|n])
        elif n<65536:prefix+=bytes([0x80|126])+struct.pack('!H',n)
        else:prefix+=bytes([0x80|127])+struct.pack('!Q',n)
        self.sock.sendall(prefix+mask+bytes(v^mask[i%4] for i,v in enumerate(data)))

    def send(self,obj):self.frame(json.dumps(obj).encode())

    def recv(self):
        body=b''
        while True:
            a,b=self.exact(2);op=a&15;fin=a&128;n=b&127
            if n==126:n=struct.unpack('!H',self.exact(2))[0]
            elif n==127:n=struct.unpack('!Q',self.exact(8))[0]
            if n>64*1024*1024:raise RuntimeError('Oversized WebSocket frame')
            mask=self.exact(4) if b&128 else None
            data=self.exact(n)
            if mask:data=bytes(v^mask[i%4] for i,v in enumerate(data))
            if op==8:raise RuntimeError('WebSocket closed')
            if op==9:self.frame(data,10);continue
            if op==10:continue
            if op not in (0,1):raise RuntimeError('Unexpected frame opcode')
            body+=data
            if fin:return json.loads(body)

    def call(self,method,params=None):
        self.counter+=1;rid=self.counter
        self.send({'id':rid,'method':method,'params':params or {}})
        deadline=time.monotonic()+45
        while time.monotonic()<deadline:
            msg=self.recv()
            if msg.get('id')==rid and 'method' not in msg:
                if 'error' in msg:raise RuntimeError(method+': '+json.dumps(msg['error']))
                return msg['result']
            if 'id' in msg and 'method' in msg:
                self.send({'id':msg['id'],'error':{'code':-32601,'message':'No interactive client actions supported'}})
            else:self.notifications.append(msg)
        raise TimeoutError(method+' timed out')

    def close(self):
        try:self.frame(b'',8)
        except OSError:pass
        self.sock.close()
