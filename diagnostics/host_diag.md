# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780746128

## Энкодер
backend=NVENC encode_ms=6 res=2560x1440 fps=60 build=11:41:18

## Кадры за интервал
- sent: 37
- skipped (статика): 0

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
