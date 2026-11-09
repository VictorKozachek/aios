# DeepSeek на bare-metal x86-64: проверяемые факты (срез: ноябрь 2026)

Все размеры файлов — из листингов Hugging Face (`/tree/main`, поле `size`), не из статей.

## 1. Размеры моделей

| Модель | Всего / активных | FP8/BF16 (ГБ) | Q8_0 | Q4_K_M | Q2_K / IQ2 | Мин. RAM |
|---|---|---|---|---|---|---|
| DeepSeek-V3 / V3-0324 | 684.5B (671B) / 37B | 688 (FP8) / 1342 (BF16) | 713 | 404 | 244 (Q2_K), 140 (UD-IQ1_S) | ~260 ГБ (Q2) / ~450 ГБ (Q4) |
| DeepSeek-V3.1 | 684.5B / 37B | 688 | 713 | 404 | 244 | то же |
| DeepSeek-V3.2 / V3.2-Exp | 685.4B / 37B | 689 | 713 | 404 | 244 | то же |
| DeepSeek-R1 | 684.5B / 37B | 688 | 713 | 404 | 244 | то же |
| DeepSeek-R1-0528 | 684.5B / 37B | 688 | 713 | 404 | 244 | то же |
| DeepSeek-V4-Pro | 1599B / 49B | 864 (репо) | — | — | — | ≥ 1 ТБ |
| **DeepSeek-V4-Flash** | 291B (284B) / **13.17B** | 159 (FP4-эксперты+FP8) | — | — | 96.8 (UD-Q2_K_XL), 82.5 (UD-IQ1_S) | **96–128 ГБ** |
| DeepSeek-V2-Lite | 15.7B / 2.4B | 31.4 (BF16) | 16.7 | 10.36 | 6.43 (Q2_K) | 8–12 ГБ |
| R1-Distill-Qwen-1.5B | 1.78B dense | 3.56 (BF16) | 1.89 | 1.12 | 0.75 | 2 ГБ |
| R1-Distill-Qwen-7B | 7.62B dense | — | 8.1 | 4.68 | 3.02 | 6 ГБ |
| R1-Distill-Qwen-14B | 14.77B dense | — | 15.7 | 8.99 | 5.77 | 12 ГБ |
| R1-Distill-Qwen-32B | 32.76B dense | — | 34.8 | 19.85 | 12.31 | 24 ГБ |
| R1-Distill-Llama-70B | 70.55B dense | — | 75.0 | 42.52 | 26.38 | 48 ГБ |

Правило RAM: веса + KV-кэш + 1–2 ГБ оверхэда; для планирования брать 1.1–1.2× от размера файла.

## 2. Пропускная способность и скорость

| Платформа | Каналы | ПСП (ГБ/с) |
|---|---|---|
| AM5, 2 DIMM, DDR5-5600 (офиц.) | 2 | 89.6 |
| AM5, 4 DIMM (офиц. DDR5-3600) | 2 | 57.6 |
| Threadripper 9000, DDR5-6400 | 4 | 204.8 |
| Threadripper Pro 9000, DDR5-6400 | 8 | 409.6 |
| EPYC 9004 (Genoa), DDR5-4800 | 12 | 460.8 |
| EPYC 9005 (Turin), DDR5-6400 | 12 | 614.4 |

Измеренное (decode, tok/s):

| Железо | Модель | Квант | Decode | Prefill |
|---|---|---|---|---|
| Ryzen 9 9955HX, 64 ГБ DDR5 | V4-Flash 284B | 1.71 bpw, 56.6 ГиБ | 9.1–9.95 (до 16 с драфтером) | 43–44 |
| Ryzen 9 9955HX | Nemotron-3.5-30B-A3B | 3.75 bpw | 25–60 | 241–307 |
| Threadripper, 8 каналов | Nemotron-30B-A3B | 3.75 bpw | 111.6 | — |
| Ryzen 7 8745H, 96 ГБ | MoE 30B-A3B | ~4 bpw | 19.5–23.4 | — |
| Ryzen 9 9950X, 96 ГБ | Llama-3.1-8B | Q4_K_M (4.9 ГБ) | 11.5 | 122 |
| Ryzen 9 9950X | Qwen3.6-27B | Q4_K_M | 3.6* | — |
| 2×Xeon 6454S + 4090D (ktransformers) | R1 671B | Q4_K_M | 13.7 | 97 |
| 2×Xeon 6454S, llama.cpp | R1 671B | Q4_K_M | 4.5 | 10.3 |
| TR Pro 3955WX 16C, 8×DDR4 | R1 671B | Q5_K_S 461.8 ГБ | 2.8 | — |
| EPYC 7K62 48C, 8 каналов | R1 671B | Q5_K_S | 4.2 | — |
| EPYC 9654, 12×DDR5-4800 | R1 671B | Q8 (688 ГБ) | 6.2 (1 сокет) / 4–5 (2 сокета) | — |
| EPYC 7552, 8×DDR4-2666 | 309-ГБ MoE | Q3_K_XL | 5.2 | — |

*оценка симулятора по сообществу, не прямое измерение.

Эффективная ПСП по фактам: 8B Q4 → 56 ГБ/с (63 % от 89.6); 27B Q4 → 58 ГБ/с; MoE → 33–155 ГБ/с (30–40 % от пика).
Формулы: dense `tok/s ≈ 0.6 × ПСП / размер`; MoE `tok/s ≈ 0.35 × ПСП / (active_params × bpw/8)`.

Расчёт для AM5 (2 DIMM, 89.6 ГБ/с): 7B Q4 ≈ 11; 14B Q4 ≈ 6.4; 32B Q4 ≈ 2.9; 70B Q2 ≈ 2.2; 70B Q4 ≈ 1.4; V4-Flash IQ1 ≈ 9; V4-Flash Q2_K_XL ≈ 7.5.

## 3. Лицензии

| Объект | Лицензия | Коммерческое использование / распространение |
|---|---|---|
| DeepSeek-V3, V3-0324 | DeepSeek License Agreement v1.0 (кастомная) | Да, но с use-based restrictions (Attachment A) и их обязательной передачей; право PRC |
| R1, R1-0528, V3.1, V3.2, V4-Pro/Flash, дистилляты | MIT | Да, без ограничений (кроме notice) |
| llama.cpp, ggml, ik_llama.cpp | MIT | Да |
| Qwen3, gpt-oss, Mistral Small 3.2 | Apache-2.0 | Да |
| Phi-4 | MIT | Да |
| Llama 3.3 | Llama 3.3 Community License (gated) | Да, но с оговоркой 700M MAU |
| Gemma 3 | Gemma Terms (gated) | Да, с use restrictions |

## 4. Движки и зависимости

llama.cpp: MIT, C++17, 541 kLOC (src 120k + ggml/src 377k + common 45k), CPU-бэкенд ggml-cpu 96k LOC, llama-mmap.cpp 916 LOC, server-http.cpp 991 LOC.

| Зависимость | Где | Что писать самим |
|---|---|---|
| `mmap/open(O_DIRECT)/madvise/sysconf` | src/llama-mmap.cpp | загрузчик GGUF в арену (можно без VM) |
| `pthread_create/join`, барьеры | ggml/src/ggml-cpu/ggml-cpu.c (37 упоминаний) | планировщик, TLS, futex |
| `std::thread/mutex`, `std::filesystem`, `new/delete`, exceptions | ggml-threading.cpp, common/* | libc++/libstdc++ порт или замена |
| `fopen/fread`, `getenv`, `clock_gettime`, `atexit`, `signal` | libc | libc-порт |
| `socket/poll` (cpp-httplib), TLS, JSON | tools/server | TCP/IP-стек + HTTP/1.1 + JSON |

Альтернативы: llamafile (Apache-2.0, Cosmopolitan — поверх системных вызовов Linux, не без ОС), candle (Rust, MIT/Apache), mistral.rs (MIT, тянет tokio/hf-hub), ik_llama.cpp (MIT, лучший CPU-MoE, AVX-512 R16-репакинг под Zen 4/5).

## 5. Zen 5

- Полный 512-битный FP-датапас на Ryzen 9000 и EPYC 9005; 256-бит на мобильных.
- AVX-512 F/CD/BW/DQ/VL/IFMA/VBMI/VBMI2/VNNI/BITALG/VPOPCNTDQ/BF16/VP2INTERSECT + AVX-VNNI. AMX — нет.
- ggml: AVX2 по умолчанию, AVX512/VBMI/VNNI/BF16 — опции сборки.
- iGPU Ryzen 9000 = RDNA2, 2 CU: для LLM бесполезен (общая ПСП DDR5). Strix Point/Halo — RDNA3.5 16–40 CU.
- NPU: у Ryzen 9000 desktop нет; XDNA2 50 TOPS только у Ryzen AI 300/400.
- ROCm: Linux-only (Ubuntu/RHEL/SLES), требует kernel-драйвер amdgpu/KFD → bare metal исключён.

## 6. Референсы

- NightRun (UEFI, Rust, MIT): грузится сразу в LLM, модели 1–4B, AVX2/FMA/F16C, без сетевого стека, framebuffer UI.
- AIOS (arXiv 2403.16971): «ОС для агентов» как слой сервисов поверх Linux — полезен дизайном планировщика/контекста, не bare metal.
- Unikraft: LibOS с POSIX/Linux-ABI, но запускается как VM.
- Redox: Rust-микроядро, POSIX-подобное; Theseus: no_std, без POSIX.
- cllm / tunguzka-llm: игрушечные bare-metal движки без HTTP.

## 7. Выводы

1. На AM5 (128 ГБ) реалистичны: V4-Flash 284B/13B @1.7–2.7 bpw (~7–10 tok/s), 30B-A3B MoE (20–30 tok/s), 32B dense Q4 (~3 tok/s), 7–14B (6–12 tok/s).
2. 671B (V3/R1) на AM5 невозможен: Q2_K = 244 ГБ > 128 ГБ. Нужен 8–12-канальный EPYC/TR Pro → 4–6 tok/s.
3. Всё упирается в ПСП (89.6 ГБ/с, КПД 35–65 %), не в FLOPs.
4. Писать самим: загрузчик GGUF, арена-аллокатор, потоки/планировщик, TCP/IP+HTTP+JSON, драйвер NIC, таймеры, ACPI/PCIe.
5. Не портировать: ROCm/CUDA/Vulkan (kernel-mode драйверы), NPU (закрытый рантайм), AMX (нет у AMD).
6. Объём: ggml-cpu 96 kLOC + llama 120 kLOC берутся как есть; заменить ~5–10 kLOC платформенного слоя.
7. V4 использует FP4-эксперты — начинать с GGUF k-quants (целочисленные).
8. Лицензии MIT у R1/V4/llama.cpp позволяют распространение; V3 требует Attachment A.
