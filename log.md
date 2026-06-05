[20:02:06] Автозапуск доступа...
[20:02:07] Firewall UDP rule already present ✓
[20:02:07] Connecting to ID server edesk.server1.everty.ru…
[20:02:07] UDP loopback test…
[20:02:07] UDP loopback: PASS ✓ — recv_from works
[20:02:07] UDP internet test (DNS → 1.1.1.1:53)…
[20:02:07] UDP internet: DNS query sent → 1.1.1.1:53
[20:02:08] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[20:02:08] === TCP probe edesk.server1.everty.ru:21116 ===
[20:02:08] TCP probe: DNS → 45.146.40.18
[20:02:08] TCP probe: connected ✓
[20:02:08] TCP probe: no greeting (server silent after connect)
[20:02:08] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:02:08] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[20:02:08] === TCP probe done ===
[20:02:08] DNS edesk.server1.everty.ru → [45.146.40.18]
[20:02:08] UDP socket local addr: 0.0.0.0:55292
[20:02:08] RegisterPk: using stable Ed25519 sign key
[20:02:08] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[20:02:08] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[20:02:08] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:02:08] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[20:02:08] UDP recv 3 bytes from 45.146.40.18:21116
[20:02:08] RegisterPkResponse  result=0
[20:02:08] Public key accepted — host is online ✓
[20:02:08] Зарегистрировано на ID сервере ✓
[20:02:08] UDP recv 2 bytes from 45.146.40.18:21116
[20:02:08] RegisterPeerResponse  request_pk=false
[20:02:08] Registered ✓ (key already on server)
[20:02:08] Зарегистрировано на ID сервере ✓
[20:02:13] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[20:02:18] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[20:02:23] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[20:02:28] UDP recv 14 bytes from 45.146.40.18:21116
[20:02:28] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[20:02:28] RelayResponse sent → edesk.server1.everty.ru (uuid=2cbc0d71-eb3e-4180-bb8a-57a4561c8863)
[20:02:28] Connecting to relay edesk.server1.everty.ru…
[20:02:28] Relay identified.
[20:02:28] Secure handshake: SignedId sent, awaiting PublicKey…
[20:02:28] Peer chose insecure (empty PublicKey) — plaintext
[20:02:28] Approval requested for (relay) (empty password)
[20:02:28] Запрос подтверждения от (relay)
[20:02:28] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[20:02:33] Approved incoming connection from (relay)
[20:02:33] EVRT: UDP сокет открыт на порту 60195
[20:02:33] EVRT: Misc{EvrtEndpoints=[192.168.0.5:60195,192.168.56.1:60195]} sent → клиент
[20:02:33] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[20:02:33] Сессия с (relay) начата
[20:02:33] Encoder loop started
[20:02:33] Encoder каскад: MF/H264 → OpenH264 → PNG
[20:02:33] TCP sender started
[20:02:33] EVRT UDP sender: ожидание punch…
[20:02:33] ★ Реальный энкодер: OpenH264-SW (2560×1440@60, первый кадр 393мс)
[20:02:33] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[20:02:36] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[20:02:36] UDP recv 3 bytes from 45.146.40.18:21116
[20:02:36] RegisterPkResponse  result=0
[20:02:36] Public key accepted — host is online ✓
[20:02:36] Зарегистрировано на ID сервере ✓
[20:02:36] UDP recv 2 bytes from 45.146.40.18:21116
[20:02:36] RegisterPeerResponse  request_pk=false
[20:02:36] Registered ✓ (key already on server)
[20:02:36] Зарегистрировано на ID сервере ✓
[20:02:39] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)
[20:02:41] EVRT: punch timeout — UDP сессия не запущена
[20:02:44] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 20s)
[20:02:49] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 15s)
[20:02:54] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 10s)
[20:02:59] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 5s)
[20:03:04] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 0s)
[20:03:04] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#6)
[20:03:04] UDP recv 3 bytes from 45.146.40.18:21116
[20:03:04] RegisterPkResponse  result=0
[20:03:04] Public key accepted — host is online ✓
[20:03:04] Зарегистрировано на ID сервере ✓
[20:03:04] UDP recv 2 bytes from 45.146.40.18:21116
[20:03:04] RegisterPeerResponse  request_pk=false
[20:03:04] Registered ✓ (key already on server)
[20:03:04] Зарегистрировано на ID сервере ✓
[20:03:09] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 23s)
[20:03:14] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 18s)
[20:03:19] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 13s)
[20:03:25] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 8s)
[20:03:30] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 3s)
[20:03:33] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#8)
[20:03:33] UDP recv 3 bytes from 45.146.40.18:21116
[20:03:33] RegisterPkResponse  result=0
[20:03:33] Public key accepted — host is online ✓
[20:03:33] Зарегистрировано на ID сервере ✓
[20:03:33] UDP recv 2 bytes from 45.146.40.18:21116
[20:03:33] RegisterPeerResponse  request_pk=false
[20:03:33] Registered ✓ (key already on server)
[20:03:33] Зарегистрировано на ID сервере ✓
[20:03:35] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 26s)