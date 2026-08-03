# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1785272814

## Энкодер
backend=OpenH264-SW encode_ms=63 encode_avg_ms=69 capture_avg_ms=280 capture_max_ms=297 slot_avg_ms=1 change_avg_ms=0 actual_fps=12.9 sent=26 skipped=0 interval_ms=2022 res=1920x1080 fps=15 build=21:06:29

## Кадры за последний интервал
- interval_ms: 2022
- actual_fps: 12.9
- sent: 26
- skipped (статика): 0

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
