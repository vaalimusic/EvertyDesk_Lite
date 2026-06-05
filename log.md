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


1780691473  Peer msg #7: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #8: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #9: VideoFrame display 0 H264 frames=1
1780691473  Video telemetry: in=8.8 fps / 561 kbps, render=0.0 fps, codec=H264, packet=6 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=декодер/отрисовка отстаёт
1780691473  Peer msg #10: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #11: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #12: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #13: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #14: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #15: VideoFrame display 0 H264 frames=1
1780691473  Peer msg #16: VideoFrame display 0 H264 frames=1
1780691474  Peer msg #17: VideoFrame display 0 H264 frames=1
1780691474  Peer msg #18: VideoFrame display 0 H264 frames=1
1780691474  Peer msg #19: VideoFrame display 0 H264 frames=1
1780691474  Peer msg #20: VideoFrame display 0 H264 frames=1
1780691474  Frame received: h264-316665 (H264)
1780691475  Frame received: h264-649997 (H264)
1780691475  Frame received: h264-983329 (H264)
1780691476  Video telemetry: in=39.9 fps / 3717 kbps, render=38.7 fps, codec=H264, packet=1 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=live поток стабилен
1780691476  Frame received: h264-1316661 (H264)
1780691477  Frame received: h264-1983325 (H264)
1780691477  Frame received: h264-2316657 (H264)
1780691478  Stream stable at 34.4 fps — raised quality to Best
1780691478  Frame received: h264-2649989 (H264)
1780691479  Frame received: h264-2983321 (H264)
1780691479  Video telemetry: in=41.6 fps / 3225 kbps, render=40.4 fps, codec=H264, packet=15 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=live поток стабилен
1780691479  Frame received: h264-3316653 (H264)
1780691480  Frame received: h264-3649985 (H264)
1780691480  Frame received: h264-3983317 (H264)
1780691481  Frame received: h264-4316649 (H264)
1780691481  Frame received: h264-4649981 (H264)
1780691481  Video QoS feedback sent: 60 fps
1780691482  Frame received: h264-4983313 (H264)
1780691482  Video telemetry: in=37.6 fps / 3226 kbps, render=38.7 fps, codec=H264, packet=19 KB, queue=0 ms, decode=4 ms, drop=0, latency=n/a, health=live поток стабилен
1780691482  Frame received: h264-5316645 (H264)
1780691483  Frame received: h264-5649977 (H264)
1780691484  Frame received: h264-6316641 (H264)
1780691485  Frame received: h264-6983305 (H264)
1780691485  Video telemetry: in=35.9 fps / 3004 kbps, render=35.2 fps, codec=H264, packet=11 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=live поток стабилен
1780691485  Frame received: h264-7316637 (H264)
1780691486  Frame received: h264-7649969 (H264)
1780691488  Video telemetry: in=5.2 fps / 329 kbps, render=8.6 fps, codec=H264, packet=1 KB, queue=0 ms, decode=7 ms, drop=0, latency=n/a, health=низкий входящий поток: хост/сеть
1780691488  Frame received: h264-7983301 (H264)
1780691489  Frame received: h264-8316633 (H264)
1780691491  Frame received: h264-8649965 (H264)
1780691491  Video QoS feedback sent: 60 fps
1780691491  Video telemetry: in=17.0 fps / 842 kbps, render=5.3 fps, codec=H264, packet=7 KB, queue=0 ms, decode=4 ms, drop=0, latency=n/a, health=декодер/отрисовка отстаёт
1780691492  Frame received: h264-8983297 (H264)
1780691493  Frame received: h264-9316629 (H264)
1780691494  Frame received: h264-9633294 (H264)
1780691494  Frame received: h264-9649961 (H264)
1780691495  Video telemetry: in=38.4 fps / 2484 kbps, render=8.2 fps, codec=H264, packet=14 KB, queue=0 ms, decode=5 ms, drop=0, latency=n/a, health=декодер/отрисовка отстаёт
1780691495  Frame received: h264-9983293 (H264)
1780691495  Frame received: h264-10316625 (H264)
1780691496  Frame received: h264-10649957 (H264)
1780691498  Video telemetry: in=10.2 fps / 686 kbps, render=10.0 fps, codec=H264, packet=1 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=live поток стабилен
1780691498  Frame received: h264-10983289 (H264)
1780691500  Frame received: h264-11316621 (H264)
1780691501  Video telemetry: in=10.2 fps / 642 kbps, render=10.4 fps, codec=H264, packet=10 KB, queue=0 ms, decode=10 ms, drop=0, latency=n/a, health=live поток стабилен
1780691501  Video QoS feedback sent: 60 fps
1780691502  Frame received: h264-11649953 (H264)
1780691503  Frame received: h264-11983285 (H264)
1780691503  Frame received: h264-12316617 (H264)
1780691504  Frame received: h264-12649949 (H264)
1780691504  Video telemetry: in=26.4 fps / 2065 kbps, render=28.0 fps, codec=H264, packet=9 KB, queue=0 ms, decode=6 ms, drop=0, latency=n/a, health=live поток стабилен
1780691505  Frame received: h264-12983281 (H264)
1780691506  Frame received: h264-13316613 (H264)
1780691507  Frame received: h264-13649945 (H264)
1780691507  Video telemetry: in=10.0 fps / 1173 kbps, render=10.0 fps, codec=H264, packet=15 KB, queue=0 ms, decode=9 ms, drop=0, latency=n/a, health=live поток стабилен
1780691509  Frame received: h264-13983277 (H264)
1780691510  Video telemetry: in=10.8 fps / 565 kbps, render=10.8 fps, codec=H264, packet=6 KB, queue=0 ms, decode=4 ms, drop=0, latency=n/a, health=live поток стабилен
1780691511  Frame received: h264-14316609 (H264)
1780691511  Video QoS feedback sent: 60 fps
1780691513  Frame received: h264-14649941 (H264)
1780691514  Video telemetry: in=10.2 fps / 787 kbps, render=10.2 fps, codec=H264, packet=7 KB, queue=0 ms, decode=8 ms, drop=0, latency=n/a, health=live поток стабилен
1780691515  Frame received: h264-14983273 (H264)
1780691517  Frame received: h264-15316605 (H264)
1780691517  Video telemetry: in=10.7 fps / 884 kbps, render=10.7 fps, codec=H264, packet=9 KB, queue=0 ms, decode=11 ms, drop=0, latency=n/a, health=live поток стабилен
1780691517  Окно управления закрыто
1780691517  Отключено