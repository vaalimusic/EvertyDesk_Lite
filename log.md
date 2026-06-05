Connected: (relay)

Video: fps=60 bitrate=4737kbps bitrate_min=3800kbps bitrate_max=5000kbps roi_avg=50pct roi_max=100pct relief=100pct relief_min=100pct avg_packet=30432B capture_avg=0ms capture_max=2ms change_avg=0ms encode_avg=260ms encode_max=2066ms

[19:54:39] Автозапуск доступа...
[19:54:39] Firewall UDP rule already present ✓
[19:54:39] Connecting to ID server edesk.server1.everty.ru…
[19:54:39] UDP loopback test…
[19:54:39] UDP loopback: PASS ✓ — recv_from works
[19:54:39] UDP internet test (DNS → 1.1.1.1:53)…
[19:54:39] UDP internet: DNS query sent → 1.1.1.1:53
[19:54:39] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[19:54:39] === TCP probe edesk.server1.everty.ru:21116 ===
[19:54:39] TCP probe: DNS → 45.146.40.18
[19:54:39] TCP probe: connected ✓
[19:54:40] TCP probe: no greeting (server silent after connect)
[19:54:40] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:54:40] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[19:54:40] === TCP probe done ===
[19:54:40] DNS edesk.server1.everty.ru → [45.146.40.18]
[19:54:40] UDP socket local addr: 0.0.0.0:63204
[19:54:40] RegisterPk: using stable Ed25519 sign key
[19:54:40] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[19:54:40] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[19:54:40] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:54:40] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[19:54:40] UDP recv 3 bytes from 45.146.40.18:21116
[19:54:40] RegisterPkResponse  result=0
[19:54:40] Public key accepted — host is online ✓
[19:54:40] Зарегистрировано на ID сервере ✓
[19:54:40] UDP recv 2 bytes from 45.146.40.18:21116
[19:54:40] RegisterPeerResponse  request_pk=false
[19:54:40] Registered ✓ (key already on server)
[19:54:40] Зарегистрировано на ID сервере ✓
[19:54:45] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[19:54:50] UDP recv 14 bytes from 45.146.40.18:21116
[19:54:50] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[19:54:50] RelayResponse sent → edesk.server1.everty.ru (uuid=3bb085c4-9ae3-4cef-ac82-95b3aa98868d)
[19:54:50] Connecting to relay edesk.server1.everty.ru…
[19:54:50] Relay identified.
[19:54:50] Secure handshake: SignedId sent, awaiting PublicKey…
[19:54:50] Peer chose insecure (empty PublicKey) — plaintext
[19:54:50] Approval requested for (relay) (empty password)
[19:54:50] Запрос подтверждения от (relay)
[19:54:50] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[19:54:52] Approved incoming connection from (relay)
[19:54:52] EVRT: UDP сокет открыт на порту 51542
[19:54:52] EVRT: Misc{EvrtEndpoints=[192.168.0.5:51542,192.168.56.1:51542]} sent → клиент
[19:54:52] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[19:54:52] Сессия с (relay) начата
[19:54:52] Encoder loop started
[19:54:52] Encoder каскад: MF/H264 → OpenH264 → PNG
[19:54:52] TCP sender started
[19:54:52] EVRT UDP sender: ожидание punch…
[19:54:55] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[19:55:00] EVRT: punch timeout — UDP сессия не запущена
[19:55:00] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[19:55:05] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 3s)
[19:55:08] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[19:55:08] UDP recv 3 bytes from 45.146.40.18:21116
[19:55:08] RegisterPkResponse  result=0
[19:55:08] Public key accepted — host is online ✓
[19:55:08] Зарегистрировано на ID сервере ✓
[19:55:08] UDP recv 2 bytes from 45.146.40.18:21116
[19:55:08] RegisterPeerResponse  request_pk=false
[19:55:08] Registered ✓ (key already on server)
[19:55:08] Зарегистрировано на ID сервере ✓
[19:55:10] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)
[19:55:16] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 20s)
[19:55:21] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 15s)
[19:55:26] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 10s)
[19:55:31] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 5s)
[19:55:36] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 0s)
[19:55:36] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#6)
[19:55:36] UDP recv 3 bytes from 45.146.40.18:21116
[19:55:36] RegisterPkResponse  result=0
[19:55:36] Public key accepted — host is online ✓
[19:55:36] Зарегистрировано на ID сервере ✓
[19:55:36] UDP recv 2 bytes from 45.146.40.18:21116
[19:55:36] RegisterPeerResponse  request_pk=false
[19:55:36] Registered ✓ (key already on server)
[19:55:36] Зарегистрировано на ID сервере ✓
[19:55:41] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 23s)
[19:55:46] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 18s)
[19:55:51] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 13s)
[19:55:56] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 8s)
[19:56:02] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 3s)
[19:56:05] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#8)
[19:56:05] UDP recv 3 bytes from 45.146.40.18:21116
[19:56:05] RegisterPkResponse  result=0
[19:56:05] Public key accepted — host is online ✓
[19:56:05] Зарегистрировано на ID сервере ✓
[19:56:05] UDP recv 2 bytes from 45.146.40.18:21116
[19:56:05] RegisterPeerResponse  request_pk=false
[19:56:05] Registered ✓ (key already on server)
[19:56:05] Зарегистрировано на ID сервере ✓
[19:56:07] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 26s)
[19:56:12] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 21s)
[19:56:17] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 16s)
[19:56:22] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 11s)
[19:56:27] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 5s)