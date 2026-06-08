// Direct WebSocket test for the harness - no external dependencies
const net = require('net');
const crypto = require('crypto');

// WebSocket client implementation using raw TCP
class WebSocket {
  constructor(url) {
    this.url = new URL(url);
    this.socket = null;
    this.onmessage = null;
    this.onopen = null;
    this.onclose = null;
    this._buffer = Buffer.alloc(0);
  }

  connect() {
    return new Promise((resolve, reject) => {
      this.socket = net.createConnection(this.url.port, this.url.hostname, () => {
        // Send HTTP upgrade request
        const key = crypto.randomBytes(16).toString('base64');
        const req = [
          `GET ${this.url.pathname} HTTP/1.1`,
          `Host: ${this.url.host}`,
          `Upgrade: websocket`,
          `Connection: Upgrade`,
          `Sec-WebSocket-Key: ${key}`,
          `Sec-WebSocket-Version: 13`,
          '',
          ''
        ].join('\r\n');
        this.socket.write(req);
      });

      this.socket.on('data', (data) => {
        this._buffer = Buffer.concat([this._buffer, data]);

        if (!this._handshakeDone) {
          // Check for HTTP 101 response
          const headerEnd = this._buffer.indexOf('\r\n\r\n');
          if (headerEnd !== -1) {
            const header = this._buffer.slice(0, headerEnd).toString();
            if (header.includes('101 Switching Protocols')) {
              this._handshakeDone = true;
              this._buffer = this._buffer.slice(headerEnd + 4);
              if (this.onopen) this.onopen();
              resolve();
            } else {
              reject(new Error('Handshake failed: ' + header));
            }
          }
        }

        // Process frames
        while (this._handshakeDone && this._buffer.length >= 2) {
          const firstByte = this._buffer[0];
          const secondByte = this._buffer[1];
          const opcode = firstByte & 0x0f;
          const masked = (secondByte & 0x80) !== 0;
          let payloadLen = secondByte & 0x7f;
          let offset = 2;

          if (payloadLen === 126) {
            if (this._buffer.length < 4) break;
            payloadLen = this._buffer.readUInt16BE(2);
            offset = 4;
          } else if (payloadLen === 127) {
            if (this._buffer.length < 10) break;
            payloadLen = Number(this._buffer.readBigUInt64BE(2));
            offset = 10;
          }

          const maskLen = masked ? 4 : 0;
          if (this._buffer.length < offset + maskLen + payloadLen) break;

          let maskKey = null;
          if (masked) {
            maskKey = this._buffer.slice(offset, offset + 4);
            offset += 4;
          }

          let payload = this._buffer.slice(offset, offset + payloadLen);
          if (maskKey) {
            for (let i = 0; i < payload.length; i++) {
              payload[i] ^= maskKey[i % 4];
            }
          }

          this._buffer = this._buffer.slice(offset + payloadLen);

          if (opcode === 0x8) {
            // Close frame
            if (this.onclose) this.onclose();
            return;
          } else if (opcode === 0x9) {
            // Ping - send pong
            this._sendFrame(0xa, Buffer.alloc(0));
          } else if (opcode === 0x1 || opcode === 0x2) {
            // Text or binary
            if (this.onmessage) {
              this.onmessage({ data: payload.toString() });
            }
          }
        }
      });

      this.socket.on('error', reject);
      this.socket.on('close', () => {
        if (this.onclose) this.onclose();
      });
    });
  }

  _sendFrame(opcode, payload) {
    const len = payload.length;
    let header;
    if (len < 126) {
      header = Buffer.alloc(2);
      header[0] = 0x80 | opcode;
      header[1] = len;
    } else if (len < 65536) {
      header = Buffer.alloc(4);
      header[0] = 0x80 | opcode;
      header[1] = 126;
      header.writeUInt16BE(len, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = 0x80 | opcode;
      header[1] = 127;
      header.writeBigUInt64BE(BigInt(len), 2);
    }
    this.socket.write(Buffer.concat([header, payload]));
  }

  send(data) {
    const payload = Buffer.from(data);
    this._sendFrame(0x1, payload);
  }

  close() {
    this._sendFrame(0x8, Buffer.alloc(0));
    this.socket.end();
  }
}

// Test sequence
async function main() {
  console.log('Connecting to harness...');
  const ws = new WebSocket('ws://127.0.0.1:2341');

  ws.onopen = () => {
    console.log('Connected!');
  };

  ws.onmessage = (msg) => {
    console.log('MSG:', msg.data);
    try {
      const data = JSON.parse(msg.data);
      if (data.type === 'welcome') {
        console.log('Got welcome, launching app...');
        ws.send(JSON.stringify({
          type: 'launch',
          app_id: 'direct-test-1',
          splash_body: 'Label { text: "Hello World" height: 30 draw_text.color: #x2ecc71 draw_text.text_style.font_size: 20 }'
        }));
      } else if (data.type === 'status') {
        console.log('Status update:', data);
        // Check for error after a short delay
        setTimeout(() => {
          console.log('Test complete, sending exit...');
          ws.send(JSON.stringify({ type: 'exit' }));
          setTimeout(() => process.exit(0), 500);
        }, 2000);
      } else if (data.type === 'error') {
        console.log('ERROR:', data);
        console.log('Test failed - error:', data.message);
        ws.send(JSON.stringify({ type: 'exit' }));
        setTimeout(() => process.exit(1), 500);
      } else if (data.type === 'user_response') {
        console.log('User response:', data);
      }
    } catch (e) {
      console.log('Parse error:', e.message);
    }
  };

  ws.onclose = () => {
    console.log('Disconnected');
  };

  await ws.connect();
}

main().catch(e => {
  console.error('Fatal:', e.message);
  process.exit(1);
});
