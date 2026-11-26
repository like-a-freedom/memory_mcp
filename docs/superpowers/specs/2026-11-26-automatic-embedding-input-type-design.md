# Автоматический выбор input_type для эмбеддингов

Статус: реализация выполнена; live smoke NVIDIA и сравнительная оценка качества ожидают доступа к API. Исходные условия ниже описывают состояние до реализации.

## Требование

Поддержать обычные модели и модели с разными режимами индексации/поиска без новых настроек пользователя. Для NVIDIA Nemotron документы должны отправляться с `input_type: "passage"`, поисковые запросы — с `input_type: "query"`. Пользователь по-прежнему выбирает provider, endpoint и model; переключатель `input_type`, отдельный NVIDIA provider и новый MCP tool не нужны.

## Проверенные исходные условия

- Hosted API [Nemotron 3.5 Embed 1B](https://docs.api.nvidia.com/nim/reference/nvidia-nemotron-3_5-embed-1b-infer) требует `passage/query`; канонический model ID — `nvidia/nemotron-3.5-embed-1b`. Подчёркивание в URL страницы не является частью model ID. При отсутствии `dimensions` API возвращает 2048 компонент. Проверено 26 ноября 2026 года.
- У self-hosted NIM другая поверхность и другие model IDs: [примеры NVIDIA](https://docs.nvidia.com/nim/nemo-retriever/embedding/latest/getting-started.html) используют `nvidia/Nemotron-3.5-Embed-1B-BF16`. Правила hosted API нельзя автоматически приписывать всем моделям на NVIDIA endpoint.
- В `embedding/providers.rs` уже есть `EmbeddingProvider::embed` для документов и `embed_query` для запросов. Default `embed_query` вызывает `embed`; локальная E5 переопределяет его.
- В `embedding/service.rs` уже есть маршрутизация Document/Query; query cache и фоновые задания разделены и учитывают signature. Переписывать этот механизм ради NVIDIA не требуется.
- `embedding/providers/remote.rs` отправляет только `model` и `input` и в рабочем запросе, и в dimension probe. У OpenAI-compatible провайдера нет собственного `embed_query`.
- `config/embedding.rs::build_embedding_signature` учитывает provider/model/base URL/dimension, а для local-candle также `e5-query-passage-v2`. Удалённый input contract сейчас не учитывается.
- Startup, recovery, CLI reembed, HTTP composition и eval adapter должны получать один согласованный результат определения модели. Сейчас некоторые пути заново строят signature из dimension.

Это проверка текущего checkout, а не подтверждение поведения отдельно установленного бинарника.

## Рассмотренные подходы

1. Всегда отправлять `input_type`. Коротко, но строгие обычные OpenAI-compatible API могут отвергнуть дополнительное поле.
2. Определять всё пробными запросами. Помогает при явной валидации, но HTTP 200 не доказывает, что сервер использовал поле: сервер может игнорировать его или иметь неявный default.
3. **Рекомендуется: проверенные профили + ограниченная проверка API.** Профиль даёт семантику известных моделей, проверка API покрывает неизвестные модели с явным требованием поля. Результат фиксируется до активации провайдера и построения signature.

Выбран третий подход.

## Контракт и автоматическое определение

Добавить доменный `EmbeddingInputContract` с вариантами `Symmetric` и `QueryPassageV1`. Это внутреннее описание wire contract, не пользовательская настройка. Local E5 продолжает применять существующие текстовые префиксы.

Порядок выбора для OpenAI-compatible API:

1. Найти точное совпадение model ID в небольшом реестре проверенных профилей. Изначально включить `nvidia/nemotron-3.5-embed-1b`, `nvidia/nemotron-3-embed-1b`, `nvidia/llama-nemotron-embed-1b-v2` и проверенные BF16/NVFP4 ID Nemotron 3.5 NIM. Для обычных моделей включить проверенные `text-embedding-ada-002`, `text-embedding-3-small`, `text-embedding-3-large`. Каждая запись содержит источник. Не применять `contains("embed")`, `starts_with("nvidia/")`, не считать все модели одного хоста одинаковыми. Сопоставление не меняет передаваемый model ID; дополнительные aliases добавляются только с источником.
2. Для известного `QueryPassageV1` сразу проверять `passage` и `query`, когда необходим сетевой preflight. Для известного `Symmetric` проверять прежний запрос без поля.
3. Для неизвестной модели сначала отправить прежний запрос без поля. При валидном векторе выбрать `Symmetric` как режим совместимости. Это не доказательство симметричной архитектуры неизвестной модели.
4. Только HTTP 400/422 с распознанной ошибкой «обязательный параметр input_type отсутствует» разрешает проверить `passage`, затем `query`. Оба ответа должны содержать валидные непустые конечные векторы одинаковой размерности. После этого выбрать `QueryPassageV1`.
5. Остальные ошибки не переключают контракт. В частности, 401/403, неизвестная модель, 429, timeout, 5xx, битый JSON и произвольный текст с упоминанием `input_type` не являются сигналом смены режима. Для известной passage/query модели отказ API принять поле — ошибка совместимости; удалять поле и продолжать молча нельзя.

Проверка использует фиксированный технический текст `memory_mcp embedding capability probe`, не данные памяти. Один общий deadline **10 секунд**, максимум **3 HTTP-запроса** на определение; без runtime backoff внутри preflight. Dimension получается из тех же ответов, отдельного повторного probe нет. Существующее правило dimension override сохраняется: override задаёт ожидаемую размерность, несовпадение проверяется при генерации.

### Dimension override и работа без сети

Для известного профиля + dimension override можно построить весь target без сетевых запросов, как сейчас. Для неизвестной модели dimension override не сообщает её input contract: перед семантической активацией потребуется ограниченная проверка API даже с override. При недоступности API stdio продолжает lexical/graph режим и существующий auto recovery; HTTP сохраняет текущий отказ запуска при неразрешённом deployment provider. Это намеренное уточнение прежней гарантии «любой dimension override полностью исключает сеть»; README и регрессионные проверки должны явно отражать границу. Новых параметров для обхода определения не вводить.

### Граница автоматизма

Нельзя универсально определить скрытую семантику неизвестной модели, если API молча принимает запросы и не объявляет возможности. HTTP 200, различие двух векторов и принадлежность endpoint конкретному производителю этого не доказывают. Известные модели с таким поведением покрываются профилем; для неизвестных сохраняется прежний формат. Не обещать поддержку произвольных task-инструкций, иных названий параметров или всех будущих моделей без обновления профилей.

## Передача результата и identity

- Определять контракт один раз на создаваемый target, до активации и любых canonical vector writes. Хранить его в immutable resolved target, передавать в фабрику провайдера. Не делать автодетект на каждом embed или отдельно для каждого tenant.
- В runtime не переключать режим под существующей signature. Изменившийся endpoint contract должен пройти повторное разрешение и существующее согласование состояния; сетевой сбой сам по себе не меняет signature.
- Включить `input_contract: "input-type-query-passage-v1"` в material signature для нового wire contract. Для Symmetric сохранить прежний material побайтно; для local E5 сохранить нынешний `e5-query-passage-v2`. Источник определения (профиль или ошибка API), версия реестра и время probe в signature не входят.
- Startup, auto recovery, reembed, HTTP и evals используют одну identity. Не восстанавливать её только по model/dimension и не терять контракт при создании провайдера.
- Не создавать глобальный или durable capability cache: target и provider уже служат process-local кэшем выбранного режима. На рестарте неизвестный режим проверяется заново; signature одинакова для одинакового контракта независимо от источника определения.
- Existing query cache, context cache и task keys остаются изолированы signature. Documents продолжают идти через `embed`, queries — через `embed_query`.

## Переход существующего индекса

Смена модели или input contract при одинаковой размерности всё равно меняет signature. Существующие startup guards, backfill-only-missing и явный `reembed` остаются владельцами перехода. Обновление не должно незаметно переписывать заполненные векторы или объявлять старые совместимыми. Reembed использует `passage`, поиск после восстановления readiness — `query`. Структура таблиц и migration files не меняются.

Автоматизм относится к выбору input contract. Массовый платный rebuild не запускается автоматически из-за нового детектора; применяется существующий workflow обслуживания. Пустой индекс с известной моделью работает сразу без настройки input_type.

## Ошибки и диагностика

Классифицировать ограниченное structured error body до sanitization, сохранить только enum причины. Не анализировать `MemoryError::to_string()` или уже редактированную строку: нынешний redactor скрывает ключи, содержащие `input`. Для начала поддержать структурированные поля OpenAI-style `error.param/code` и validation-array `detail[].loc/type` с точным `input_type` и missing/required code. Неструктурированный текст не запускает negotiation. Raw body и тексты памяти не попадaют в новые события.

Событие `embedding.input_contract_resolved`: model, sanitized endpoint, contract, detection source, dimension. Событие состояния — INFO; отказ/degradation — по существующему `MemoryError::log_level()`. Для полного resolution `duration_ms` только на верхнем уровне. Метки метрик не расширять model/endpoint значениями.

## Ограничения реализации

8-tool MCP surface сохраняется; новые зависимости и изменения `Cargo.toml` не требуются. Новая логика живёт в `embedding/`, composition/recovery adapters только передают результат. Не менять пользовательские текущие правки retrieval/evals и не коммитить их вместе с этой работой.

## Приёмка

- Для NVIDIA до записи первого вектора выбран QueryPassageV1; документы, recovery и reembed отправляют passage, retrieval отправляет query.
- Обычный endpoint, отвергающий неизвестные поля, получает прежний JSON без input_type. Local E5/Ollama/disabled сохраняют свои контракты.
- Неизвестная strict-модель определяется через missing-field response без новых настроек; timeout/авторизация/rate limit не превращаются в «успешный детект».
- Новая signature не смешивается со старой даже при одинаковой размерности; обычная модель после обновления не требует ненужного reembed.
- Политика совпадает во всех composition roots и eval adapter; HTTP не повторяет probe для каждого tenant.

Дополнительные источники профилей: [Nemotron 3](https://docs.api.nvidia.com/nim/re/reference/nvidia-nemotron-3-embed-1b-infer), [Llama Nemotron v2](https://docs.api.nvidia.com/nim/reference/nvidia-llama-nemotron-embed-1b-v2-infer). Перед реализацией сверить точные model IDs и сохранённые примеры validation errors с актуальными API.
