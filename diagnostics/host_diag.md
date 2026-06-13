# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781225459

## Энкодер
backend=NVENC encode_ms=7 encode_avg_ms=6 capture_avg_ms=4 capture_max_ms=5 slot_avg_ms=0 change_avg_ms=0 actual_fps=25.5 sent=51 skipped=9 interval_ms=2001 res=2560x1440 fps=30 build=00:48:46

## Кадры за последний интервал
- interval_ms: 2001
- actual_fps: 25.5
- sent: 51
- skipped (статика): 9

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
