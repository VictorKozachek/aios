# tests/qemu-smoke — дымовой тест загрузки AIOS в CI

Задача **#012 (T6)**: CI должен падать, если ядро не дошло до маркера загрузки,
а не только если код не компилируется.

Границы задачи: этот каталог и `.github/workflows/ci.yml`. Ядро, `Makefile` и
`build/` — зона задачи #001 (T1); здесь они только вызываются.

## Файлы

| Файл | Что делает |
|---|---|
| `run.sh` | запускает QEMU (OVMF → Limine → ядро), ждёт маркер в serial/debugcon, возвращает 0/1 |
| `self-test.sh` | проверяет сам `run.sh` на фейковом QEMU — без QEMU, OVMF и образа |
| `negative-test.sh` | берёт рабочий образ, ломает ядро и требует, чтобы `run.sh` упал |
| `install-limine.sh` | скачивает официальный Limine (BOOTX64.EFI) для CI-сборки образа |

## Как работает `run.sh`

```
tests/qemu-smoke/run.sh --image build/aios.img
```

1. Проверяет, что образ существует. Нет образа — **честное падение**, а не
   «зелёный прогон на пустоте».
2. Находит OVMF (`OVMF_CODE` / `--ovmf-code`, иначе стандартные пути macOS и
   Linux), делает свежую копию `OVMF_VARS`.
3. Запускает QEMU:

   ```
   -M q35 -m 512 -drive if=pflash,...,file=OVMF_CODE
   -drive if=ide,file=build/aios.img
   -device isa-debug-exit
   -serial file:build/serial.log
   -debugcon file:build/debugcon.log -global isa-debugcon.iobase=0xE9
   -display none -monitor none -no-reboot
   ```

4. Каждую секунду ищет маркер (по умолчанию `boot: ok, halting`) в
   `build/serial.log` **или** `build/debugcon.log`. Нашёл — успех, QEMU
   останавливается.
5. Нет маркера за `--timeout` секунд, QEMU упал или завершился раньше —
   печатает хвосты обоих логов и QEMU stderr и возвращает 1. Логи остаются на
   диске для артефакта CI.

Коды возврата: `0` — маркер найден, `1` — провал, `2` — ошибка аргументов.

### `isa-debug-exit`

QEMU запускается с `-device isa-debug-exit`. Если ядро когда-нибудь начнёт
писать в порт `0x501`, QEMU завершится со статусом `(value << 1) | 1`
(например, `value = 0x10` → статус `33`). Проверка статуса включается
опцией `--expect-exit-code 33`; без неё достаточно маркера (текущее ядро
уходит в `hlt`, а не завершает QEMU).

## Локальный запуск

```bash
make image
tests/qemu-smoke/run.sh --image build/aios.img
```

Проверка самого раннера (не нужны QEMU/OVMF/образ, идёт секунды):

```bash
tests/qemu-smoke/self-test.sh
```

Негативная проверка на реальном образе (нужны QEMU, OVMF и `mtools`):

```bash
tests/qemu-smoke/negative-test.sh --image build/aios.img --timeout 60
```

`negative-test.sh` копирует образ, подменяет `/boot/kernel` пустым файлом и
требует, чтобы `run.sh` упал. Это CI-доказательство критерия приёмки: сломанное
ядро не проходит.

### Как воспроизвести «пустой BootInfo» вручную

Полная версия негативного сценария из карточки — намеренно сломать `BootInfo`
(например, обнулить карту памяти в `kernel/src/boot/*` так, чтобы ядро не
напечатало `boot: ok, halting`), собрать образ и прогнать:

```bash
make image && tests/qemu-smoke/run.sh --image build/aios.img
```

Ожидаемо: красный прогон, в логах нет маркера.

## CI

`.github/workflows/ci.yml` содержит два job'а:

- **host-tests** — `cargo test` библиотек, `cargo check` ядра под
  `x86_64-unknown-none`, а также `tests/qemu-smoke/self-test.sh`;
- **qemu-smoke** — ставит `qemu-system-x86`, `ovmf`, `mtools`, скачивает Limine
  (`install-limine.sh`), собирает образ `make image`, гоняет `run.sh` и
  `negative-test.sh`, всегда выкладывает логи артефактом `qemu-smoke-logs`.

Зависимость: зелёный `qemu-smoke` возможен только после слияния **#001**
(`make image`). До этого job честно падает на отсутствии образа — это ожидаемое
состояние, а не поломка теста.
