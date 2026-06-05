[20:30:42] Автозапуск доступа...
[20:30:42] Firewall UDP rule already present ✓
[20:30:42] Connecting to ID server edesk.server1.everty.ru…
[20:30:42] UDP loopback test…
[20:30:42] UDP loopback: PASS ✓ — recv_from works
[20:30:42] UDP internet test (DNS → 1.1.1.1:53)…
[20:30:42] UDP internet: DNS query sent → 1.1.1.1:53
[20:31:11] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[20:31:11] === TCP probe edesk.server1.everty.ru:21116 ===
[20:31:11] TCP probe: DNS → 45.146.40.18
[20:31:11] TCP probe: connected ✓
[20:31:11] TCP probe: no greeting (server silent after connect)
[20:31:11] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:31:11] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[20:31:11] === TCP probe done ===
[20:31:11] DNS edesk.server1.everty.ru → [45.146.40.18]
[20:31:11] UDP socket local addr: 0.0.0.0:65153
[20:31:11] RegisterPk: using stable Ed25519 sign key
[20:31:11] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[20:31:11] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[20:31:11] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:31:11] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[20:31:11] UDP recv 3 bytes from 45.146.40.18:21116
[20:31:11] RegisterPkResponse  result=0
[20:31:11] Public key accepted — host is online ✓
[20:31:11] Зарегистрировано на ID сервере ✓
[20:31:11] UDP recv 2 bytes from 45.146.40.18:21116
[20:31:11] RegisterPeerResponse  request_pk=false
[20:31:11] Registered ✓ (key already on server)
[20:31:11] Зарегистрировано на ID сервере ✓
[20:31:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[20:31:11] UDP recv 15 bytes from 45.146.40.18:21116
[20:31:11] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[20:31:11] RelayResponse sent → edesk.server1.everty.ru (uuid=f2dd9d29-7a31-4bf6-b44d-807b22af4ecc)
[20:31:11] Connecting to relay edesk.server1.everty.ru…
[20:31:11] Relay identified.
[20:31:11] Secure handshake: SignedId sent, awaiting PublicKey…
[20:31:11] Peer chose insecure (empty PublicKey) — plaintext
[20:31:11] Approval requested for (relay) (empty password)
[20:31:11] Запрос подтверждения от (relay)
[20:31:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[20:31:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[20:31:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[20:31:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[20:31:11] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[20:31:11] UDP recv 3 bytes from 45.146.40.18:21116
[20:31:11] RegisterPkResponse  result=0
[20:31:11] Public key accepted — host is online ✓
[20:31:11] Зарегистрировано на ID сервере ✓
[20:31:11] UDP recv 2 bytes from 45.146.40.18:21116
[20:31:11] RegisterPeerResponse  request_pk=false
[20:31:11] Registered ✓ (key already on server)
[20:31:11] Зарегистрировано на ID сервере ✓
[20:31:12] Approved incoming connection from (relay)
[20:31:12] EVRT: UDP сокет открыт на порту 59415
[20:31:12] EVRT: Misc{EvrtEndpoints=[192.168.0.5:59415,192.168.56.1:59415]} sent → клиент
[20:31:12] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[20:31:12] Сессия с (relay) начата
[20:31:12] Encoder loop started
[20:31:12] Encoder каскад: NVENC/H264 → MF/H264 → OpenH264 → PNG
[20:31:12] TCP sender started
[20:31:12] EVRT UDP sender: ожидание punch…
[20:31:12] ★ Реальный энкодер: NVENC (2560×1440@60, первый кадр 145мс)
[20:31:14] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)
[20:31:19] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 20s)
[20:31:20] EVRT: punch timeout — UDP сессия не запущена
[20:31:24] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 15s)
[20:31:29] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 10s)
[20:31:34] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 5s)
[20:31:39] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 0s)
[20:31:40] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#6)
[20:31:40] UDP recv 3 bytes from 45.146.40.18:21116
[20:31:40] RegisterPkResponse  result=0
[20:31:40] Public key accepted — host is online ✓
[20:31:40] Зарегистрировано на ID сервере ✓
[20:31:40] UDP recv 2 bytes from 45.146.40.18:21116
[20:31:40] RegisterPeerResponse  request_pk=false
[20:31:40] Registered ✓ (key already on server)
[20:31:40] Зарегистрировано на ID сервере ✓
[20:31:44] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 23s)
[20:31:50] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 18s)
[20:31:55] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 13s)
[20:31:57] Pipeline для (relay) завершён
[20:31:57] Encoder loop stopped
[20:31:57] Session with (relay) ended normally.
[20:31:57] Сессия с (relay) завершена 
[20:31:57] TCP sender stopped
[20:32:00] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 8s)