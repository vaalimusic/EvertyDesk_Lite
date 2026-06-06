# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780747452

## Энкодер
backend=NVENC encode_ms=7 encode_avg_ms=6 capture_avg_ms=4 capture_max_ms=5 slot_avg_ms=0 change_avg_ms=0 actual_fps=54.5 sent=109 skipped=11 interval_ms=2000 res=2560x1440 fps=60 build=12:03:25

## Кадры за последний интервал
- interval_ms: 2000
- actual_fps: 54.5
- sent: 109
- skipped (статика): 11

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
