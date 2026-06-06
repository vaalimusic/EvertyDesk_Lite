# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780746618

## Энкодер
backend=NVENC encode_ms=6 encode_avg_ms=6 capture_avg_ms=42 capture_max_ms=47 slot_avg_ms=0 change_avg_ms=0 actual_fps=16.2 sent=33 skipped=0 interval_ms=2032 res=2560x1440 fps=60 build=11:49:29

## Кадры за последний интервал
- interval_ms: 2032
- actual_fps: 16.2
- sent: 33
- skipped (статика): 0

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
