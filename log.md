[19:48:11] Автозапуск доступа...
[19:48:12] Firewall UDP rule already present ✓
[19:48:12] Connecting to ID server edesk.server1.everty.ru…
[19:48:12] UDP loopback test…
[19:48:12] UDP loopback: PASS ✓ — recv_from works
[19:48:12] UDP internet test (DNS → 1.1.1.1:53)…
[19:48:12] UDP internet: DNS query sent → 1.1.1.1:53
[19:49:38] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[19:49:38] === TCP probe edesk.server1.everty.ru:21116 ===
[19:49:38] TCP probe: DNS → 45.146.40.18
[19:49:38] TCP probe: connected ✓
[19:49:38] TCP probe: no greeting (server silent after connect)
[19:49:38] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:49:38] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[19:49:38] === TCP probe done ===
[19:49:38] DNS edesk.server1.everty.ru → [45.146.40.18]
[19:49:38] UDP socket local addr: 0.0.0.0:58923
[19:49:38] RegisterPk: using stable Ed25519 sign key
[19:49:38] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[19:49:38] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[19:49:38] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:49:38] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[19:49:38] UDP recv 5 bytes from 45.146.40.18:21116
[19:49:38] RegisterPkResponse  result=4
[19:49:38] Error: RegisterPk rejected (result=4)  Retrying in 10 s…
[19:49:38] Connecting to ID server edesk.server1.everty.ru…
[19:49:38] UDP loopback test…
[19:49:38] UDP loopback: PASS ✓ — recv_from works
[19:49:38] UDP internet test (DNS → 1.1.1.1:53)…
[19:49:38] UDP internet: DNS query sent → 1.1.1.1:53
[19:49:38] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[19:49:38] === TCP probe edesk.server1.everty.ru:21116 ===
[19:49:38] TCP probe: DNS → 45.146.40.18
[19:49:38] TCP probe: connected ✓
[19:49:38] TCP probe: no greeting (server silent after connect)
[19:49:38] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:49:38] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[19:49:38] === TCP probe done ===
[19:49:38] DNS edesk.server1.everty.ru → [45.146.40.18]
[19:49:38] UDP socket local addr: 0.0.0.0:52097
[19:49:38] RegisterPk: using stable Ed25519 sign key
[19:49:38] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[19:49:38] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[19:49:38] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[19:49:38] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[19:49:38] UDP recv 3 bytes from 45.146.40.18:21116
[19:49:38] RegisterPkResponse  result=0
[19:49:38] Public key accepted — host is online ✓
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] UDP recv 2 bytes from 45.146.40.18:21116
[19:49:38] RegisterPeerResponse  request_pk=false
[19:49:38] Registered ✓ (key already on server)
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 13s)
[19:49:38] UDP recv 15 bytes from 45.146.40.18:21116
[19:49:38] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[19:49:38] RelayResponse sent → edesk.server1.everty.ru (uuid=3a7fe259-24ad-4d8d-8eaa-8c4aa700db66)
[19:49:38] Connecting to relay edesk.server1.everty.ru…
[19:49:38] Relay identified.
[19:49:38] Secure handshake: SignedId sent, awaiting PublicKey…
[19:49:38] Peer chose insecure (empty PublicKey) — plaintext
[19:49:38] Approval requested for (relay) (empty password)
[19:49:38] Запрос подтверждения от (relay)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 8s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[19:49:38] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[19:49:38] UDP recv 3 bytes from 45.146.40.18:21116
[19:49:38] RegisterPkResponse  result=0
[19:49:38] Public key accepted — host is online ✓
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] UDP recv 2 bytes from 45.146.40.18:21116
[19:49:38] RegisterPeerResponse  request_pk=false
[19:49:38] Registered ✓ (key already on server)
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 26s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 20s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 15s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 10s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 5s)
[19:49:38] UDP recv 137 bytes from 45.146.40.18:21116
[19:49:38] RequestRelay incoming target=454055949 relay=edesk.server1.everty.ru uuid=ba0899db-e6e9-4af4-b6d1-e3f10aed00e6
[19:49:38] RelayResponse sent → edesk.server1.everty.ru (uuid=ba0899db-e6e9-4af4-b6d1-e3f10aed00e6)
[19:49:38] Входящий запрос от 454055949
[19:49:38] Connecting to relay edesk.server1.everty.ru…
[19:49:38] Relay identified.
[19:49:38] Secure handshake: SignedId sent, awaiting PublicKey…
[19:49:38] Peer chose insecure (empty PublicKey) — plaintext
[19:49:38] Approval requested for 454055949 (empty password)
[19:49:38] Запрос подтверждения от 454055949
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 4×, следующий heartbeat через 0s)
[19:49:38] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#6)
[19:49:38] UDP recv 3 bytes from 45.146.40.18:21116
[19:49:38] RegisterPkResponse  result=0
[19:49:38] Public key accepted — host is online ✓
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] UDP recv 2 bytes from 45.146.40.18:21116
[19:49:38] RegisterPeerResponse  request_pk=false
[19:49:38] Registered ✓ (key already on server)
[19:49:38] Зарегистрировано на ID сервере ✓
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 23s)
[19:49:38] Session with (relay) error: Incoming approval timed out
[19:49:38] Сессия с (relay) завершена 
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 18s)
[19:49:38] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 13s)
[19:49:40] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 8s)
[19:49:40] Approved incoming connection from 454055949
[19:49:40] EVRT: UDP сокет открыт на порту 56549
[19:49:40] EVRT: Misc endpoints send failed: TCP write failed: Программа на вашем хост-компьютере разорвала установленное подключение. (os error 10053)
[19:49:40] Auth OK для 454055949. Pipeline старт: 30fps quality=70%
[19:49:40] Сессия с 454055949 начата
[19:49:40] Pipeline для 454055949 завершён
[19:49:40] TCP sender started
[19:49:40] TCP sender stopped
[19:49:40] Encoder loop started
[19:49:40] Encoder каскад: MF/H264 → OpenH264 → PNG
[19:49:40] Encoder loop stopped
[19:49:40] EVRT UDP sender: ожидание punch…
[19:49:40] EVRT: punch timeout — UDP сессия не запущена
[19:49:40] Session with 454055949 ended normally.
[19:49:40] Сессия с 454055949 завершена 
[19:49:45] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 6×, следующий heartbeat через 2s)
[19:49:48] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#8)
[19:49:48] UDP recv 3 bytes from 45.146.40.18:21116
[19:49:48] RegisterPkResponse  result=0
[19:49:48] Public key accepted — host is online ✓
[19:49:48] Зарегистрировано на ID сервере ✓
[19:49:48] UDP recv 2 bytes from 45.146.40.18:21116
[19:49:48] RegisterPeerResponse  request_pk=false
[19:49:48] Registered ✓ (key already on server)
[19:49:48] Зарегистрировано на ID сервере ✓
[19:49:50] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 26s)
[19:49:54] UDP recv 15 bytes from 45.146.40.18:21116
[19:49:54] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[19:49:54] RelayResponse sent → edesk.server1.everty.ru (uuid=2d875372-92dd-4ca2-b0b1-200e9d9671df)
[19:49:54] Connecting to relay edesk.server1.everty.ru…
[19:49:54] Relay identified.
[19:49:54] Secure handshake: SignedId sent, awaiting PublicKey…
[19:49:54] Peer chose insecure (empty PublicKey) — plaintext
[19:49:54] Password OK ✓
[19:49:54] Approval requested for (relay)
[19:49:54] Запрос подтверждения от (relay)
[19:49:55] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 21s)
[19:49:56] Approved incoming connection from (relay)
[19:49:56] EVRT: UDP сокет открыт на порту 64011
[19:49:56] EVRT: Misc{EvrtEndpoints=[192.168.0.5:64011,192.168.56.1:64011,169.254.43.65:64011,169.254.123.78:64011,169.254.160.60:64011,169.254.176.136:64011,169.254.222.120:64011,169.254.231.215:64011]} sent → клиент
[19:49:56] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[19:49:56] Сессия с (relay) начата
[19:49:56] TCP sender started
[19:49:56] Encoder loop started
[19:49:56] Encoder каскад: MF/H264 → OpenH264 → PNG
[19:49:56] EVRT UDP sender: ожидание punch…
[19:50:00] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 15s)
[19:50:04] EVRT: punch timeout — UDP сессия не запущена
[19:50:05] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 10s)
[19:50:07] Pipeline для (relay) завершён
[19:50:07] Session with (relay) ended normally.
[19:50:07] Сессия с (relay) завершена 
[19:50:07] Encoder loop stopped
[19:50:08] TCP sender stopped
[19:50:11] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 8×, следующий heartbeat через 5s)