Connected: (relay)

Video: fps=60 bitrate=4503kbps bitrate_min=3800kbps bitrate_max=5000kbps roi_avg=41pct roi_max=100pct relief=100pct relief_min=100pct avg_packet=27534B capture_avg=0ms capture_max=2ms change_avg=0ms encode_avg=190ms encode_max=467ms

[19:42:05] Автозапуск доступа...
[19:42:05] Firewall UDP rule already present ✓
[19:42:05] Connecting to ID server edesk.server1.everty.ru…
[19:42:05] UDP loopback test…
[19:42:05] UDP loopback: PASS ✓ — recv_from works
[19:42:05] UDP internet test (DNS → 1.1.1.1:53)…
[19:42:05] UDP internet: DNS query sent → 1.1.1.1:53
[19:42:07] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[19:42:07] === TCP probe edesk.server1.everty.ru:21116 ===
[19:42:07] TCP probe: DNS → 45.146.40.18
[19:42:07] TCP probe: connected ✓
[19:42:07] TCP probe: no greeting (server silent after connect)
[19:42:07] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:42:07] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[19:42:07] === TCP probe done ===
[19:42:07] DNS edesk.server1.everty.ru → [45.146.40.18]
[19:42:07] UDP socket local addr: 0.0.0.0:58835
[19:42:07] RegisterPk: using stable Ed25519 sign key
[19:42:07] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[19:42:07] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[19:42:07] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:42:07] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[19:42:07] UDP recv 3 bytes from 45.146.40.18:21116
[19:42:07] RegisterPkResponse  result=0
[19:42:07] Public key accepted — host is online ✓
[19:42:07] Зарегистрировано на ID сервере ✓
[19:42:07] UDP recv 2 bytes from 45.146.40.18:21116
[19:42:07] RegisterPeerResponse  request_pk=false
[19:42:07] Registered ✓ (key already on server)
[19:42:07] Зарегистрировано на ID сервере ✓
[19:42:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[19:42:16] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[19:42:16] UDP recv 15 bytes from 45.146.40.18:21116
[19:42:16] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[19:42:16] RelayResponse sent → edesk.server1.everty.ru (uuid=fe590b17-8b1b-4728-9ce3-755ded750dfd)
[19:42:16] Connecting to relay edesk.server1.everty.ru…
[19:42:16] Relay identified.
[19:42:16] Secure handshake: SignedId sent, awaiting PublicKey…
[19:42:16] Peer chose insecure (empty PublicKey) — plaintext
[19:42:16] Approval requested for (relay) (empty password)
[19:42:16] Запрос подтверждения от (relay)
[19:42:18] Approved incoming connection from (relay)
[19:42:18] EVRT: UDP сокет открыт на порту 51274
[19:42:18] EVRT: Misc{EvrtEndpoints=[192.168.0.5:51274,192.168.56.1:51274,169.254.43.65:51274,169.254.123.78:51274,169.254.160.60:51274,169.254.176.136:51274,169.254.222.120:51274,169.254.231.215:51274]} sent → клиент
[19:42:18] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[19:42:18] Сессия с (relay) начата
[19:42:18] Encoder loop started
[19:42:18] Encoder каскад: MF/H265 → OpenH264 → PNG
[19:42:18] TCP sender started
[19:42:18] EVRT UDP sender: ожидание punch…
[19:42:21] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[19:42:26] EVRT: punch timeout — UDP сессия не запущена
[19:42:26] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[19:42:32] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[19:42:34] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[19:42:34] UDP recv 3 bytes from 45.146.40.18:21116
[19:42:34] RegisterPkResponse  result=0
[19:42:34] Public key accepted — host is online ✓
[19:42:34] Зарегистрировано на ID сервере ✓
[19:42:34] UDP recv 2 bytes from 45.146.40.18:21116
[19:42:34] RegisterPeerResponse  request_pk=false
[19:42:34] Registered ✓ (key already on server)
[19:42:34] Зарегистрировано на ID сервере ✓
[19:42:37] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)