# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1781485587

## Энкодер
backend=NVENC encode_ms=7 encode_avg_ms=7 capture_avg_ms=88 capture_max_ms=102 slot_avg_ms=0 change_avg_ms=0 actual_fps=0.8 sent=2 skipped=18 interval_ms=2500 res=2560x1440 fps=30 build=00:59:42

## Кадры за последний интервал
- interval_ms: 2500
- actual_fps: 0.8
- sent: 2
- skipped (статика): 18

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
