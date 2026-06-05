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


1780692131  Video telemetry: in=8.7 fps / 729 kbps, render=0.1 fps, codec=H264, packet=9 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=декодер/отрисовка отстаёт
1780692132  Peer msg #10: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #11: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #12: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #13: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #14: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #15: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #16: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #17: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #18: VideoFrame display 0 H264 frames=1
1780692132  Peer msg #19: VideoFrame display 0 H264 frames=1
1780692133  Peer msg #20: VideoFrame display 0 H264 frames=1
1780692133  Frame received: h264-316665 (H264)
1780692134  Frame received: h264-649997 (H264)
1780692134  Frame received: h264-983329 (H264)
1780692135  Video telemetry: in=40.7 fps / 3313 kbps, render=33.2 fps, codec=H264, packet=21 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=live поток стабилен
1780692135  Frame received: h264-1316661 (H264)
1780692135  Frame received: h264-1649993 (H264)
1780692136  Frame received: h264-2316657 (H264)
1780692137  Stream stable at 39.0 fps — raised quality to Best
1780692137  Frame received: h264-2649989 (H264)
1780692137  Frame received: h264-2983321 (H264)
1780692138  Video telemetry: in=42.1 fps / 3595 kbps, render=39.1 fps, codec=H264, packet=7 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=live поток стабилен
1780692138  Frame received: h264-3649985 (H264)
1780692140  Video QoS feedback sent: 60 fps
1780692142  Video telemetry: in=2.3 fps / 106 kbps, render=13.0 fps, codec=H264, packet=5 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692142  Frame received: h264-3983317 (H264)
1780692144  Frame received: h264-4316649 (H264)
1780692145  Video telemetry: in=10.4 fps / 593 kbps, render=10.4 fps, codec=H264, packet=6 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=live поток стабилен
1780692146  Frame received: h264-4649981 (H264)
1780692148  Frame received: h264-4983313 (H264)
1780692148  Video telemetry: in=8.4 fps / 512 kbps, render=11.0 fps, codec=H264, packet=8 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=live поток стабилен
1780692150  Video QoS feedback sent: 60 fps
1780692150  Frame received: h264-5316645 (H264)
1780692151  Video telemetry: in=9.1 fps / 415 kbps, render=9.8 fps, codec=H264, packet=6 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=live поток стабилен
1780692152  Frame received: h264-5649977 (H264)
1780692154  Video telemetry: in=3.6 fps / 332 kbps, render=3.6 fps, codec=H264, packet=6 KB, queue=0 ms, decode=4 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692157  Frame received: h264-5983309 (H264)
1780692158  Video telemetry: in=10.3 fps / 557 kbps, render=4.8 fps, codec=H264, packet=6 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=live поток стабилен
1780692160  Frame received: h264-6316641 (H264)
1780692160  Video QoS feedback sent: 60 fps
1780692162  Video telemetry: in=4.2 fps / 131 kbps, render=6.9 fps, codec=H264, packet=3 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692163  Frame received: h264-6649973 (H264)
1780692165  Video telemetry: in=2.2 fps / 212 kbps, render=8.7 fps, codec=H264, packet=14 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692165  Frame received: h264-6983305 (H264)
1780692169  Video telemetry: in=4.6 fps / 491 kbps, render=7.8 fps, codec=H264, packet=19 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692169  Frame received: h264-7316637 (H264)
1780692170  Video QoS feedback sent: 60 fps
1780692173  Video telemetry: in=5.9 fps / 393 kbps, render=1.3 fps, codec=H264, packet=3 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692173  Frame received: h264-7649969 (H264)
1780692177  Video telemetry: in=3.2 fps / 278 kbps, render=6.5 fps, codec=H264, packet=5 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692178  Frame received: h264-7983301 (H264)
1780692180  Video QoS feedback sent: 60 fps
1780692180  Video telemetry: in=4.2 fps / 274 kbps, render=10.2 fps, codec=H264, packet=20 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692181  Frame received: h264-8316633 (H264)
1780692183  Video telemetry: in=2.4 fps / 174 kbps, render=5.7 fps, codec=H264, packet=10 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692185  Frame received: h264-8649965 (H264)
1780692187  Video telemetry: in=10.5 fps / 501 kbps, render=10.4 fps, codec=H264, packet=5 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=live поток стабилен
1780692188  Frame received: h264-8983297 (H264)
1780692190  Video QoS feedback sent: 60 fps
1780692192  Video telemetry: in=2.3 fps / 165 kbps, render=10.4 fps, codec=H264, packet=0 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692196  Video telemetry: in=0.5 fps / 52 kbps, render=0.5 fps, codec=H264, packet=14 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692198  Frame received: h264-9316629 (H264)
1780692199  Video telemetry: in=3.7 fps / 69 kbps, render=1.4 fps, codec=H264, packet=2 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692200  Video QoS feedback sent: 60 fps
1780692203  Video telemetry: in=2.0 fps / 82 kbps, render=0.7 fps, codec=H264, packet=12 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692205  Frame received: h264-9633294 (H264)
1780692205  Frame received: h264-9649961 (H264)
1780692208  Video telemetry: in=2.8 fps / 171 kbps, render=12.0 fps, codec=H264, packet=13 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692208  Frame received: h264-9983293 (H264)
1780692210  Video QoS feedback sent: 60 fps
1780692211  Video telemetry: in=0.8 fps / 3 kbps, render=5.0 fps, codec=H264, packet=13 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692212  Frame received: h264-10316625 (H264)
1780692212  Frame received: h264-10649957 (H264)
1780692216  Video telemetry: in=2.7 fps / 214 kbps, render=10.1 fps, codec=H264, packet=13 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692216  Frame received: h264-10983289 (H264)
1780692220  Video telemetry: in=0.5 fps / 44 kbps, render=0.5 fps, codec=H264, packet=10 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692220  Video QoS feedback sent: 60 fps
1780692223  Video telemetry: in=0.9 fps / 2 kbps, render=1.0 fps, codec=H264, packet=11 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780692226  Error: TCP read header failed: failed to fill whole buffer