# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781475677

## Энкодер
backend=NVENC encode_ms=6 encode_avg_ms=6 capture_avg_ms=4 capture_max_ms=6 slot_avg_ms=0 change_avg_ms=0 actual_fps=24.5 sent=49 skipped=11 interval_ms=2001 res=2560x1440 fps=30 build=22:20:08

## Кадры за последний интервал
- interval_ms: 2001
- actual_fps: 24.5
- sent: 49
- skipped (статика): 11

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
