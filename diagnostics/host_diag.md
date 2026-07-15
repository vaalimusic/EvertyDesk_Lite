# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1782247767

## Энкодер
backend=NVENC encode_ms=7 encode_avg_ms=7 capture_avg_ms=0 capture_max_ms=4 slot_avg_ms=0 change_avg_ms=0 actual_fps=0.8 sent=2 skipped=72 interval_ms=2467 res=2560x1440 fps=30 build=19:57:39

## Кадры за последний интервал
- interval_ms: 2467
- actual_fps: 0.8
- sent: 2
- skipped (статика): 72

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
