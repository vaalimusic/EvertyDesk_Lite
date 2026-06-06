# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780747020

## Энкодер
backend=NVENC encode_ms=7 encode_avg_ms=8 capture_avg_ms=5 capture_max_ms=8 slot_avg_ms=0 change_avg_ms=0 actual_fps=40.2 sent=81 skipped=2 interval_ms=2013 res=2560x1440 fps=60 build=11:56:07

## Кадры за последний интервал
- interval_ms: 2013
- actual_fps: 40.2
- sent: 81
- skipped (статика): 2

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
