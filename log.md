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


1780688491  Frame received: h264-0 (H264)
1780688493  Peer msg #4: VideoFrame display 0 H264 frames=1
1780688493  Video telemetry: in=0.7 fps / 258 kbps, render=0.1 fps, codec=H264, packet=37 KB, queue=0 ms, decode=129 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688493  Peer msg #5: VideoFrame display 0 H264 frames=1
1780688495  Peer msg #6: VideoFrame display 0 H264 frames=1
1780688495  Peer msg #7: VideoFrame display 0 H264 frames=1
1780688495  Peer msg #8: VideoFrame display 0 H264 frames=1
1780688496  Peer msg #9: VideoFrame display 0 H264 frames=1
1780688496  Peer msg #10: VideoFrame display 0 H264 frames=1
1780688496  Video telemetry: in=4.6 fps / 1207 kbps, render=0.9 fps, codec=H264, packet=5 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688496  Peer msg #11: VideoFrame display 0 H264 frames=1
1780688497  Peer msg #12: VideoFrame display 0 H264 frames=1
1780688497  Peer msg #13: VideoFrame display 0 H264 frames=1
1780688497  Peer msg #14: VideoFrame display 0 H264 frames=1
1780688498  Peer msg #15: VideoFrame display 0 H264 frames=1
1780688498  Peer msg #16: VideoFrame display 0 H264 frames=1
1780688499  Peer msg #17: VideoFrame display 0 H264 frames=1
1780688499  Peer msg #18: VideoFrame display 0 H264 frames=1
1780688499  Video telemetry: in=2.7 fps / 491 kbps, render=2.3 fps, codec=H264, packet=18 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688499  Video QoS feedback sent: 60 fps
1780688499  Peer msg #19: VideoFrame display 0 H264 frames=1
1780688500  Peer msg #20: VideoFrame display 0 H264 frames=1
1780688500  Frame received: h264-316665 (H264)
1780688503  Video telemetry: in=4.2 fps / 873 kbps, render=1.3 fps, codec=H264, packet=93 KB, queue=0 ms, decode=29 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688503  Окно управления закрыто
1780688503  Отключено
1780688530  Session started for 454055949
1780688530  Подключение к 454055949
1780688530  5% - Validating input
1780688530  15% - Validating server public key
1780688530  30% - Connecting to ID server
1780688530  45% - Connecting to Relay server
1780688536  52% - UDP NAT probe unavailable; direct UDP disabled for this attempt (45.146.40.18:21116 attempt 12: recv failed: Resource temporarily unavailable (os error 35))
1780688536  60% - Sending RustDesk PunchHoleRequest (EVRT probe)
1780688536  80% - Waiting for rendezvous response
1780688536  85% - Rendezvous protobuf response decoded
1780688536  86% - Rendezvous selected relay; no direct candidate returned
1780688536  88% - Using relay reservation from rendezvous response
1780688536  92% - Opening relay stream
1780688536  96% - Waiting for peer secure/login response
1780688538  Connected: authorized; peer info: hostname=VAALIMUSIC, platform=windows, version=1.4.6; screenshot/control channel ready
1780688538  Displays detected: 1
1780688538  Display subscribed; waiting for first frame
1780688538  Peer msg #1: Misc
1780688538  Video QoS feedback sent: 60 fps
1780688538  Peer msg #2: Misc
1780688539  Peer msg #3: VideoFrame display 0 H264 frames=1
1780688539  Live video stream active; using low-latency frame path
1780688539  Frame received: h264-0 (H264)
1780688541  Peer msg #4: VideoFrame display 0 H264 frames=1
1780688541  Video telemetry: in=0.8 fps / 317 kbps, render=0.1 fps, codec=H264, packet=92 KB, queue=0 ms, decode=29 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688541  Peer msg #5: VideoFrame display 0 H264 frames=1
1780688542  Peer msg #6: VideoFrame display 0 H264 frames=1
1780688542  Peer msg #7: VideoFrame display 0 H264 frames=1
1780688542  Peer msg #8: VideoFrame display 0 H264 frames=1
1780688542  Peer msg #9: VideoFrame display 0 H264 frames=1
1780688543  Peer msg #10: VideoFrame display 0 H264 frames=1
1780688543  Peer msg #11: VideoFrame display 0 H264 frames=1
1780688543  Peer msg #12: VideoFrame display 0 H264 frames=1
1780688544  Peer msg #13: VideoFrame display 0 H264 frames=1
1780688544  Peer msg #14: VideoFrame display 0 H264 frames=1
1780688544  Peer msg #15: VideoFrame display 0 H264 frames=1
1780688544  Peer msg #16: VideoFrame display 0 H264 frames=1
1780688544  Peer msg #17: VideoFrame display 0 H264 frames=1
1780688544  Video telemetry: in=4.8 fps / 1205 kbps, render=2.3 fps, codec=H264, packet=24 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688545  Peer msg #18: VideoFrame display 0 H264 frames=1
1780688545  Peer msg #19: VideoFrame display 0 H264 frames=1
1780688545  Peer msg #20: VideoFrame display 0 H264 frames=1
1780688546  Frame received: h264-316665 (H264)
1780688548  Video QoS feedback sent: 60 fps
1780688548  Video telemetry: in=3.3 fps / 247 kbps, render=1.8 fps, codec=H264, packet=14 KB, queue=0 ms, decode=6 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688551  Video telemetry: in=2.5 fps / 163 kbps, render=0.7 fps, codec=H264, packet=6 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688554  Frame received: h264-649997 (H264)
1780688554  Video telemetry: in=3.7 fps / 343 kbps, render=2.6 fps, codec=H264, packet=9 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688558  Video telemetry: in=3.5 fps / 262 kbps, render=1.7 fps, codec=H264, packet=11 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688558  Video QoS feedback sent: 60 fps
1780688561  Video telemetry: in=1.4 fps / 488 kbps, render=2.5 fps, codec=H264, packet=9 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780688564  Frame received: h264-983329 (H264)
1780688564  Окно управления закрыто
1780688564  Отключено