Video: fps=60 bitrate=4756kbps bitrate_min=4700kbps bitrate_max=5000kbps roi_avg=13pct roi_max=100pct relief=100pct relief_min=100pct avg_packet=8361B capture_avg=0ms capture_max=1ms change_avg=0ms encode_avg=8ms encode_max=164ms
[20:26:08] Автозапуск доступа...
[20:26:08] Firewall UDP rule already present ✓
[20:26:08] Connecting to ID server edesk.server1.everty.ru…
[20:26:08] UDP loopback test…
[20:26:08] UDP loopback: PASS ✓ — recv_from works
[20:26:08] UDP internet test (DNS → 1.1.1.1:53)…
[20:26:08] UDP internet: DNS query sent → 1.1.1.1:53
[20:26:14] UDP internet: PASS ✓ — got 96B from 1.1.1.1:53 (inbound UDP from internet works!)
[20:26:14] === TCP probe edesk.server1.everty.ru:21116 ===
[20:26:14] TCP probe: DNS → 45.146.40.18
[20:26:14] TCP probe: connected ✓
[20:26:14] TCP probe: no greeting (server silent after connect)
[20:26:14] TCP probe: sending framed RegisterPeer 13B: 32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:26:14] TCP probe: server closed after our message (EOF) — normal for TCP 21116
[20:26:14] === TCP probe done ===
[20:26:14] DNS edesk.server1.everty.ru → [45.146.40.18]
[20:26:14] UDP socket local addr: 0.0.0.0:51079
[20:26:14] RegisterPk: using stable Ed25519 sign key
[20:26:14] RegisterPk packet: 65 bytes  hex=7A 3F 0A 09 34 35 34 30 35 35 39 34 39 12 10 B3 50 52 BB 74 …(65 total)
[20:26:14] RegisterPk sent → edesk.server1.everty.ru:21116  id=454055949  (#1)
[20:26:14] RegisterPeer packet: 13 bytes  hex=32 0B 0A 09 34 35 34 30 35 35 39 34 39
[20:26:14] RegisterPeer sent → edesk.server1.everty.ru:21116  id=454055949  (#2)
[20:26:14] UDP recv 3 bytes from 45.146.40.18:21116
[20:26:14] RegisterPkResponse  result=0
[20:26:14] Public key accepted — host is online ✓
[20:26:14] Зарегистрировано на ID сервере ✓
[20:26:14] UDP recv 2 bytes from 45.146.40.18:21116
[20:26:14] RegisterPeerResponse  request_pk=false
[20:26:14] Registered ✓ (key already on server)
[20:26:14] Зарегистрировано на ID сервере ✓
[20:26:14] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 23s)
[20:26:17] UDP recv 14 bytes from 45.146.40.18:21116
[20:26:17] FetchLocalAddr incoming  relay=edesk.server1.everty.ru
[20:26:17] RelayResponse sent → edesk.server1.everty.ru (uuid=f954ea24-e1ca-418c-a9ff-7af9e6415135)
[20:26:17] Connecting to relay edesk.server1.everty.ru…
[20:26:17] Relay identified.
[20:26:17] Secure handshake: SignedId sent, awaiting PublicKey…
[20:26:17] Peer chose insecure (empty PublicKey) — plaintext
[20:26:17] Approval requested for (relay) (empty password)
[20:26:17] Запрос подтверждения от (relay)
[20:26:18] Approved incoming connection from (relay)
[20:26:18] EVRT: UDP сокет открыт на порту 60486
[20:26:18] EVRT: Misc{EvrtEndpoints=[192.168.0.5:60486,192.168.56.1:60486]} sent → клиент
[20:26:18] Auth OK для (relay). Pipeline старт: 30fps quality=70%
[20:26:18] Сессия с (relay) начата
[20:26:18] Encoder loop started
[20:26:18] Encoder каскад: NVENC/H265 → MF/H264 → OpenH264 → PNG
[20:26:18] TCP sender started
[20:26:18] EVRT UDP sender: ожидание punch…
[20:26:18] ★ Реальный энкодер: NVENC (2560×1440@30, первый кадр 137мс)
[20:26:20] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 18s)
[20:26:25] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 12s)
[20:26:26] EVRT: punch timeout — UDP сессия не запущена
[20:26:30] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 7s)
[20:26:35] Ожидание ответа от edesk.server1.everty.ru:21116 … (отправлено 2×, следующий heartbeat через 2s)
[20:26:37] Heartbeat: RegisterPk+RegisterPeer sent → edesk.server1.everty.ru:21116 (#4)
[20:26:37] UDP recv 3 bytes from 45.146.40.18:21116
[20:26:37] RegisterPkResponse  result=0
[20:26:37] Public key accepted — host is online ✓
[20:26:37] Зарегистрировано на ID сервере ✓
[20:26:37] UDP recv 2 bytes from 45.146.40.18:21116
[20:26:37] RegisterPeerResponse  request_pk=false
[20:26:37] Registered ✓ (key already on server)
[20:26:37] Зарегистрировано на ID сервере ✓


1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691208  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691209  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691210  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Окно управления закрыто
1780691211  Server sent H265, but hardware H265 decode is unavailable; requesting fallback
1780691211  Отключено