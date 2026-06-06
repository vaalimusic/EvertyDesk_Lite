# Хост-диагностика EvertyDesk Lite (живая)

Обновлено: unix 1780743699

## Энкодер
backend=NVENC encode_ms=6 res=2560x1440 fps=60 build=11:00:56

## Кадры за интервал
- sent: 33
- skipped (статика): 0

> Это пишет САМ ХОСТ из encode_loop. Если файл свежий (unix растёт) и
> backend/encode_ms заполнены — хост точно на свежем билде.
> `backend=OpenH264-SW encode_ms>100` = софт (нет NVENC/MF аппаратного).
> `backend=NVENC encode_ms<10` = аппаратный RTX.
