# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781141022

## Энкодер
backend=NVENC encode_ms=6 encode_avg_ms=6 capture_avg_ms=72 capture_max_ms=88 slot_avg_ms=0 change_avg_ms=0 actual_fps=1.5 sent=3 skipped=16 interval_ms=2067 res=2560x1440 fps=30 build=01:20:42

## Кадры за последний интервал
- interval_ms: 2067
- actual_fps: 1.5
- sent: 3
- skipped (статика): 16

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
