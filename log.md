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



1780689290  60% - Sending RustDesk PunchHoleRequest (EVRT probe)
1780689290  80% - Waiting for rendezvous response
1780689290  85% - Rendezvous protobuf response decoded
1780689290  86% - Rendezvous selected relay; no direct candidate returned
1780689290  88% - Using relay reservation from rendezvous response
1780689290  92% - Opening relay stream
1780689290  96% - Waiting for peer secure/login response
1780689292  Connected: authorized; peer info: hostname=VAALIMUSIC, platform=windows, version=1.4.6; screenshot/control channel ready
1780689292  Displays detected: 1
1780689292  Display subscribed; waiting for first frame
1780689292  Peer msg #1: Misc
1780689292  Video QoS feedback sent: 60 fps
1780689292  Peer msg #2: Misc
1780689293  Peer msg #3: VideoFrame display 0 H264 frames=1
1780689293  Live video stream active; using low-latency frame path
1780689293  Frame received: h264-0 (H264)
1780689295  Peer msg #4: VideoFrame display 0 H264 frames=1
1780689295  Video telemetry: in=0.7 fps / 564 kbps, render=0.1 fps, codec=H264, packet=104 KB, queue=0 ms, decode=97 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689296  Peer msg #5: VideoFrame display 0 H264 frames=1
1780689296  Peer msg #6: VideoFrame display 0 H264 frames=1
1780689297  Peer msg #7: VideoFrame display 0 H264 frames=1
1780689297  Peer msg #8: VideoFrame display 0 H264 frames=1
1780689297  Peer msg #9: VideoFrame display 0 H264 frames=1
1780689297  Peer msg #10: VideoFrame display 0 H264 frames=1
1780689298  Peer msg #11: VideoFrame display 0 H264 frames=1
1780689298  Peer msg #12: VideoFrame display 0 H264 frames=1
1780689298  Video telemetry: in=3.7 fps / 653 kbps, render=4.8 fps, codec=H264, packet=27 KB, queue=0 ms, decode=4 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689298  Peer msg #13: VideoFrame display 0 H264 frames=1
1780689298  Peer msg #14: VideoFrame display 0 H264 frames=1
1780689298  Peer msg #15: VideoFrame display 0 H264 frames=1
1780689299  Peer msg #16: VideoFrame display 0 H264 frames=1
1780689299  Peer msg #17: VideoFrame display 0 H264 frames=1
1780689299  Peer msg #18: VideoFrame display 0 H264 frames=1
1780689300  Peer msg #19: VideoFrame display 0 H264 frames=1
1780689300  Peer msg #20: VideoFrame display 0 H264 frames=1
1780689300  Frame received: h264-316665 (H264)
1780689301  Video QoS feedback sent: 60 fps
1780689301  Video telemetry: in=3.5 fps / 1031 kbps, render=6.5 fps, codec=H264, packet=86 KB, queue=0 ms, decode=29 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689305  Video telemetry: in=0.8 fps / 331 kbps, render=4.8 fps, codec=H264, packet=16 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689306  Frame received: h264-649997 (H264)
1780689308  Video telemetry: in=4.5 fps / 394 kbps, render=4.1 fps, codec=H264, packet=5 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689309  Frame received: h264-983329 (H264)
1780689311  Video QoS feedback sent: 60 fps
1780689312  Video telemetry: in=3.8 fps / 442 kbps, render=6.1 fps, codec=H264, packet=15 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689314  Frame received: h264-1316661 (H264)
1780689315  Video telemetry: in=2.5 fps / 937 kbps, render=3.7 fps, codec=H264, packet=9 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689320  Video telemetry: in=0.9 fps / 802 kbps, render=2.9 fps, codec=H264, packet=111 KB, queue=0 ms, decode=32 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689321  Video QoS feedback sent: 60 fps
1780689324  Video telemetry: in=0.5 fps / 436 kbps, render=2.0 fps, codec=H264, packet=115 KB, queue=0 ms, decode=27 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689328  Video telemetry: in=0.5 fps / 441 kbps, render=0.5 fps, codec=H264, packet=115 KB, queue=0 ms, decode=31 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689331  Video QoS feedback sent: 60 fps
1780689333  Video telemetry: in=0.4 fps / 419 kbps, render=0.5 fps, codec=H264, packet=115 KB, queue=0 ms, decode=13 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689334  Frame received: h264-1649993 (H264)
1780689337  Video telemetry: in=1.4 fps / 736 kbps, render=2.1 fps, codec=H264, packet=9 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689340  Video telemetry: in=1.1 fps / 217 kbps, render=2.1 fps, codec=H264, packet=111 KB, queue=0 ms, decode=28 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689341  Video QoS feedback sent: 60 fps
1780689344  Video telemetry: in=0.5 fps / 451 kbps, render=1.4 fps, codec=H264, packet=114 KB, queue=0 ms, decode=28 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689348  Video telemetry: in=0.5 fps / 427 kbps, render=0.5 fps, codec=H264, packet=114 KB, queue=0 ms, decode=32 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689352  Video QoS feedback sent: 60 fps
1780689352  Video telemetry: in=0.5 fps / 445 kbps, render=0.5 fps, codec=H264, packet=114 KB, queue=0 ms, decode=30 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689357  Video telemetry: in=0.5 fps / 438 kbps, render=0.5 fps, codec=H264, packet=114 KB, queue=0 ms, decode=32 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689361  Video telemetry: in=0.5 fps / 30 kbps, render=0.5 fps, codec=H264, packet=114 KB, queue=0 ms, decode=30 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689361  Frame received: h264-1983325 (H264)
1780689362  Video QoS feedback sent: 60 fps
1780689365  Video telemetry: in=1.4 fps / 530 kbps, render=0.8 fps, codec=H264, packet=10 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689370  Video telemetry: in=0.5 fps / 464 kbps, render=0.4 fps, codec=H264, packet=116 KB, queue=0 ms, decode=17 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689372  Video QoS feedback sent: 60 fps
1780689373  Video telemetry: in=1.4 fps / 100 kbps, render=1.4 fps, codec=H264, packet=7 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689377  Video telemetry: in=0.5 fps / 134 kbps, render=4.4 fps, codec=H264, packet=11 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689377  Frame received: h264-2316657 (H264)
1780689380  Video telemetry: in=1.2 fps / 58 kbps, render=0.9 fps, codec=H264, packet=77 KB, queue=0 ms, decode=31 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689382  Video QoS feedback sent: 60 fps
1780689384  Video telemetry: in=3.2 fps / 854 kbps, render=0.9 fps, codec=H264, packet=9 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689387  Video telemetry: in=3.3 fps / 702 kbps, render=0.5 fps, codec=H264, packet=9 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689389  Frame received: h264-2649989 (H264)
1780689390  Video telemetry: in=2.9 fps / 651 kbps, render=5.7 fps, codec=H264, packet=10 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689392  Video QoS feedback sent: 60 fps
1780689393  Video telemetry: in=4.2 fps / 343 kbps, render=0.5 fps, codec=H264, packet=9 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780689394  Окно управления закрыто
1780689394  Отключено