# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781487891

## Энкодер
backend=NVENC encode_ms=5 encode_avg_ms=8 capture_avg_ms=10 capture_max_ms=80 slot_avg_ms=0 change_avg_ms=0 actual_fps=17.7 sent=36 skipped=17 interval_ms=2030 res=2560x1440 fps=30 build=01:43:22

## Кадры за последний интервал
- interval_ms: 2030
- actual_fps: 17.7
- sent: 36
- skipped (статика): 17

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
