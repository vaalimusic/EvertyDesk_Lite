Connected: (relay)

Video: fps=60 bitrate=5000kbps bitrate_min=5000kbps bitrate_max=5000kbps roi_avg=14pct roi_max=100pct relief=100pct relief_min=100pct avg_packet=6178B capture_avg=0ms capture_max=2ms change_avg=0ms encode_avg=5ms encode_max=6ms

[20:41:39] Автозапуск доступа...
[20:41:40] Firewall UDP rule already present ✓
[20:41:40] Connecting to ID server edesk.server1.everty.ru…
[20:41:40] UDP loopback test…
[20:41:40] UDP loopback: PASS ✓ — recv_from works
[20:41:40] UDP internet test (DNS → 1.1.1.1:53)…
[20:41:40] UDP internet: DNS query sent → 1.1.1.1:53
[20:42:04] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[20:42:04] === TCP probe edesk.server1.everty.ru:21116 ===
[20:42:04] TCP probe: DNS → 45.146.40.18
[20:42:04] TCP probe: connected ✓
[20:42:04] TCP probe: no greeting (server silent after connect)
[20:42:04] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:42:04] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[20:42:04] === TCP probe done ===
[20:42:04] DNS edesk.server1.everty.ru → [45.146.40.18]
[20:42:04] UDP socket local addr: 0.0.0.0:53203
[20:42:04] RegisterPk: using stable Ed25519 sign key
[20:42:04] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[20:42:04] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[20:42:04] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:42:04] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[20:42:04] UDP recv 3 bytes from 45.146.40.18:21116
[20:42:04] RegisterPkResponse  result=0
[20:42:04] Public key accepted — host is online ✓
[20:42:04] Зарегистрировано на ID сервере ✓
[20:42:04] UDP recv 2 bytes from 45.146.40.18:21116
[20:42:04] RegisterPeerResponse  request_pk=false
[20:42:04] Registered ✓ (key already on server)
[20:42:04] Зарегистрировано на ID сервере ✓
[20:42:04] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[20:42:04] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[20:42:04] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[20:42:04] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[20:42:06] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[20:42:09] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[20:42:09] UDP recv 3 bytes from 45.146.40.18:21116
[20:42:09] RegisterPkResponse  result=0
[20:42:09] Public key accepted — host is online ✓
[20:42:09] Зарегистрировано на ID сервере ✓
[20:42:09] UDP recv 2 bytes from 45.146.40.18:21116
[20:42:09] RegisterPeerResponse  request_pk=false
[20:42:09] Registered ✓ (key already on server)
[20:42:09] Зарегистрировано на ID сервере ✓
[20:42:09] UDP recv 15 bytes from 45.146.40.18:21116
[20:42:09] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[20:42:09] RelayResponse sent → edesk.server1.everty.ru (uuid=7942595b-0746-43c6-b6ee-68e500b81521)
[20:42:09] Connecting to relay edesk.server1.everty.ru…
[20:42:09] Relay identified.
[20:42:09] Secure handshake: SignedId sent, awaiting PublicKey…
[20:42:09] Peer chose insecure (empty PublicKey) — plaintext
[20:42:09] Approval requested for (relay) (empty password)
[20:42:09] Запрос подтверждения от (relay)
[20:42:10] Approved incoming connection from (relay)
[20:42:10] EVRT: UDP сокет открыт на порту 51956
[20:42:10] EVRT: Misc{EvrtEndpoints=[192.168.0.5:51956,192.168.56.1:51956]} sent → клиент
[20:42:10] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[20:42:10] Сессия с (relay) начата
[20:42:10] Encoder loop started
[20:42:10] Encoder каскад: NVENC/H264 → MF/H264 → OpenH264 → PNG
[20:42:10] TCP sender started
[20:42:10] EVRT UDP sender: ожидание punch…
[20:42:11] ★ Реальный энкодер: NVENC (2560×1440@60, первый кадр 152мс)
[20:42:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)
[20:42:16] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 21s)
[20:42:19] EVRT: punch timeout — UDP сессия не запущена
[20:42:21] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 15s)
[20:42:26] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 10s)
[20:42:32] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 5s)
[20:42:37] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 0s)
[20:42:37] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#6)
[20:42:37] UDP recv 3 bytes from 45.146.40.18:21116
[20:42:37] RegisterPkResponse  result=0
[20:42:37] Public key accepted — host is online ✓
[20:42:37] Зарегистрировано на ID сервере ✓
[20:42:37] UDP recv 2 bytes from 45.146.40.18:21116
[20:42:37] RegisterPeerResponse  request_pk=false
[20:42:37] Registered ✓ (key already on server)
[20:42:37] Зарегистрировано на ID сервере ✓
[20:42:42] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 23s)
[20:42:47] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 18s)
[20:42:52] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 13s)
[20:42:57] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 8s)
[20:43:02] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 3s)
[20:43:05] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#8)
[20:43:05] UDP recv 3 bytes from 45.146.40.18:21116
[20:43:05] RegisterPkResponse  result=0
[20:43:05] Public key accepted — host is online ✓
[20:43:05] Зарегистрировано на ID сервере ✓
[20:43:05] UDP recv 2 bytes from 45.146.40.18:21116
[20:43:05] RegisterPeerResponse  request_pk=false
[20:43:05] Registered ✓ (key already on server)
[20:43:05] Зарегистрировано на ID сервере ✓
[20:43:07] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 26s)
[20:43:12] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 21s)
[20:43:17] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 16s)
[20:43:23] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 11s)