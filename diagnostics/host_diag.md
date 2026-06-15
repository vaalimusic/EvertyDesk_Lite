# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781490134

## Энкодер
backend=NVENC encode_ms=6 encode_avg_ms=6 capture_avg_ms=3 capture_max_ms=5 slot_avg_ms=0 change_avg_ms=0 actual_fps=26.5 sent=53 skipped=7 interval_ms=2000 res=2560x1440 fps=30 build=02:21:24

## Кадры за последний интервал
- interval_ms: 2000
- actual_fps: 26.5
- sent: 53
- skipped (статика): 7

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
