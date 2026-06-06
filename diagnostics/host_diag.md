# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780745320

## Энкодер
backend=NVENC encode_ms=6 res=2560x1440 fps=60 build=11:27:50

## Кадры за интервал
- sent: 44
- skipped (статика): 0

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
