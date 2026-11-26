# Качество памяти: результаты исправлений, 26 ноября 2026

Задачи реализации и итоговой оценки плана 1–7 выполнены в зафиксированном
объёме. Два исходных full-history провала проходят **2/2**, native gate — **8/8**.
Это не закрытие всех проблем качества: независимый ответный контракт дал
**4/7**, ложная предпосылка — **0/3**. C2 **NO-GO**.
Список остаточных доработок и критериев приёмки приведён ниже.

Все новые исполнения локальны на Apple Silicon. Детерминированные оценки не
требуют модели. Один финальный семикейсовый capture использует standalone MLX
и заменяемые loopback endpoints: без LM Studio runtime, Codex и remote inference.
Путь к уже сохранённым весам в кэше `lm-studio` не является runtime-зависимостью.

## Текущие метрики

| Измерение | Результат | Граница вывода |
| --- | ---: | --- |
| Исходные full-store dropdown / toolbar, без модели | **2/2** | Исходные source IDs и полные exact spans в одном final item; read≤30с/process≤45с |
| Исходные reviewed spans в actual reader contexts, одна формула v2 | Предыдущий полный capture **4/7** → финальный **4/7** | Буквальная полнота одного заранее выбранного источника; не ответная точность |
| Официальный pinned scorer | **4/7**, оценено **7/7** | Один exposed development capture; не независимая оценка пользы |
| Независимый закрытый answer contract | **4/7** | Неизменённые accepted answers, все семь в знаменателе |
| Exact phrase-set, четыре положительных случая | **4/4** | Лишние фразы отклоняются; совпадение множества, не свободный semantic judge |
| False-premise ответы | **0/3** | Проверяются отдельно от положительных ответов |
| Полный authentic alternative browse-tabs блок в одном reader item | **да** | Дополнительный exposed контроль; исходный gold-span не заменён |
| Native temporal/authority gate | **8/8** | no_regressions, 0invalid/0unobserved на этом наборе |
| CLI query latency, семь случаев | median **38.411с**, max **44.897с** | Private-store copy + startup + read, три concurrent workers |
| Completion cap256 / отсутствие boxed | **3 / 3** из7 | Не установлена единственная причина неверного ответа |
| Ошибки reader/judge, завершение native scoring | **0 терминальных reader / 0 judge**; оценены7/7 | SDK retries исходного профиля разрешены; это не число всех HTTP-attempt failures |
| Точные цитаты и source IDs в исходном reader adapter | **не наблюдаются** | Adapter доставляет text-only items; native source binding проверяется отдельно |

[Финальный анализ](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/original-seven-post-topical/final-analysis.json),
[raw prompts/readers/scored rows](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/original-seven-post-topical/scores/),
[проверка неизменности](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/original-seven-post-topical/final-integrity.json).
Предыдущий полный raw256 capture:3/7. Старый архив raw256 остановился после двух
scored rows:2/7 там — нижняя граница, а не полная точность. Архив concise512
использовал другой prompt/cap. Нельзя выдавать эти числа за matched improvement.
Оба последних полных capture используют исходные prompts/модели/snapshot; даже
их различие описательное, потому что семь кейсов раскрыты, нагрузка хоста не
контролируется, независимой подтверждающей выборки нет.

## Все семь случаев

| ID | Gold-span: предыдущий → финальный | Native score / независимый verdict | Разбор |
| --- | --- | --- | --- |
| `0c6b0c60` | да → да | 1.0 / correct | Исходный dropdown; полный набор time ranges |
| `6d975a08` | нет → нет | 1.0 / correct | Ответ Forums, Tags, Alphabetical верен; исходный gold-source остаётся отдельным miss |
| `89a20f48` | да → да | 0.0 / incorrect | Отказ от ложной предпосылки; source/use/format различаются |
| `922a0c56` | нет → нет | 0.0 / incorrect | Отказ от ложной предпосылки; потеря reviewed span требует отдельной трассы |
| `d6da3938` | нет → нет | 1.0 / correct | Соседняя кнопка; full-span absence не равна отсутствию ответа |
| `f43926d9` | да → да | 0.0 / incorrect | Отказ от ложной предпосылки в directory |
| `f61a096f` | да → да | 1.0 / correct | Исходный полный toolbar/count |

Для каталога отдельный `topical-tabs-preflight` на тех же42 source/test hashes
подтверждает точный authentic4ae source episode и полную тройку ссылок во всех
восьми стадиях. В финальном original6d native receipt альтернативный блок полон
в одном final item; стадийные snapshots этого original-source контроля фильтруют
исходный gold и не используются как трасса альтернативы.
Финальный original6d single-gold-source metric остаётся
**miss**; он не скрыт новым alternative control. Ни текстовое совпадение, ни хороший aggregate не
доказывают источник или правильное цитирование. Во всех трёх false-premise
ответах достигнут cap256 и отсутствует финальный boxed answer; в двух при этом
полный reviewed span доставлен. Это локализует часть проблемы после retrieval,
но не разделяет влияние prompt, лимита генерации и поведения reader.

## Детерминированное исправление полной истории

`topical-tabs-final` использует неизменённую целую100-trajectory историю, source
identities, вопросы и spans. Не увеличены candidate/source caps или excerpt2048B.
Каждое чтение идёт на полной disposable копии; copy time вне read/process часов.

| Исходный случай | Read | Процесс | Приёмка |
| --- | ---: | ---: | --- |
| Dropdown | 21.149с | 36.632с | complete exact source, pass |
| Toolbar | 26.852с | 40.860с | complete exact source, pass |

Есть formation → ranked → subject-scoped → admitted → bounded → episode →
selected → packed receipts. Late Ready сверх30с и process elapsed сверх45с
отклоняются независимо от exit code. Source/binary/snapshot hashes закреплены;
исходный store не переформирован. Неудачные и source-drift attempts сохранены,
не используются как свежая приёмка. [Реестр всех attempts](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/candidate-summary.json).

Найденный после первого model capture дефект упаковки воспроизведён отдельно:
короткие link-list tabs обрывались; вложенный список и предшествующая длинная
button group скрывали evidence unit; центрирование первой соседней группы
обрезало следующую. Наконец, unrelated sidebar list, совпавший лишь по роли
`list`, делал union слишком широким. Каждый обнаруженный дефект имел public
runtime RED→GREEN; отдельный bound-control проверяет отказ от превышения2048B.
Имена независимых fixtures — Archive/Collections/Labels/Recent activity,
не benchmark answers. Контроль narrative и mixed button request сохраняет
прежний выбор. Поддержка pure tabs конечна: запрос одновременно о tabs и
button/toolbar/dropdown использует прежнюю button policy; универсальный NLP
parser и обратный mixed-tab intent не заявлены.

Исходный gold-source каталога находится на actual rank40, first-source rank40
из148 кандидатов. Следовательно, source-diversity deduplication не является
доказанным решением. Исправление не поднимает hidden gold IDs и не меняет ranking.

## Фактическая стоимость кандидатов

| Actual lexical pool исходной пары | Dropdown | Toolbar |
| --- | ---: | ---: |
| First-ID facts | 242 | 252 |
| Content UTF8 bytes |119580400|100684202|
| Index-key occurrences |2148886|1872111|
| Key UTF8 bytes |47006223|39639620|
| Exact content / key lists |96 /96|97 /97|
| Actual key-dictionary builds |96|97|
| Admission lists / raw+URL dictionaries |114 /228|150 /300|

Это один словарь на точный список **в конкретном pool/semantic form**, не один
глобально. Совпадение проверяется по самим спискам. UTF8 fields не равны wire
bytes/RSS/памяти движка. SQL hydration/reuse сохраняют independent wide-SQL oracle,
first-hit score/order, per-term limits, access/temporal policy и полные payloads.
Memo8192entries/1MiB ограничивает кэш, не обработку overflow. Indexed graph reads,
ограниченная конкурентность graph/access и episode membership matrix устраняют подтверждённую работу.

Application-poll allocation control:3669865→838002 calls (−77,2%), прежний cap1M.
Storage/background polls исключены; это fixed fixture, не RSS или full-store speed.
Для прежней pinned toolbar трассы пять ожидаемых durable access updates заняли
около6,23с в перекрывающихся волнах, собственно упаковка0,328с. Их нельзя
складывать как последовательные SQL или считать доказательством engine-причины.
[Стадийная стоимость прежней трассы](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/source-refresh-14d14b46/read-cost-breakdown.json).

## Честность измерения и проверки

Span metric v2 проверяет целый verified span в одном item: raw, прежняя newline/tab
форма либо точная JSON-string serialization. Нет fuzzy match, recursive decode,
склейки item или новых accepted answers. CLI runtime RED выявил ложное отсутствие
toolbar в v1; старые результаты сохранены, одна v2 формула применена к обеим
сторонам.11 CLI controls входят в discovery. Literal presence не равна читаемости.

До финальных model outputs protocol review выявил и закрыл два пробела через
четыре subprocess controls RED→GREEN: native gate обязан соответствовать SHA
этого неизменённого release; при reader/judge failure частичный отчёт сохраняет
все7 scheduled cases и доступные prompt-source metrics, а acceptance отклоняется.
Пятый положительный control воспроизводит старый complete capture без изменения
его3/7. Вход также требует завершённой original primary pair и snapshot integrity.
[Контроли](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/verification/protocol-controls-complete-control.log),
[регистрация до capture](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/original-seven-post-topical/analysis-registration.json).

Server logs также показывают реальные повторные POST и BrokenPipe traceback lines.
Это отдельно от terminal reader/judge case status; HTTP200 headers не доказывают
завершённую доставку. Нет связи каждой попытки с конкретным case, поэтому такие
строки не превращаются в точное число failed SDK attempts. В финальном reader log15 POST records и3 BrokenPipe exception lines против
прежних11 и2; judge —3 POST и0 таких строк в обоих captures. Фиксированный клиент
имеет30s timeout и до10 retries; их overhead включён в реальное время capture.
Scored-pass wall вырос с399,251с до547,628с; median CLI query — с37,160с до38,411с.
Ускорение этим сравнением не подтверждается; нагрузка хоста не контролировалась.
[Наблюдения запросов и их границы](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/runtime-attempt-observations.json).

В immutable upstream system prompt наблюдается U+0008: Python `\b` в инструкции
об отказе превратился в backspace. Это реально присутствует во всех7 actual prompts,
но не доказывает причину ответных ошибок. Для сопоставимости upstream prompt
сохранён; его исправление требует отдельного зарегистрированного протокола.
[Наблюдение и хеши](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/upstream-format-observation.json).

| Проверка финального дерева | Результат |
| --- | --- |
| Production crate | 2856 passed /0failed /45ignored |
| Eval-harness | 929 passed /0failed /22ignored |
| Scoped source/resource,7targets | 111 passed /0failed |
| Conversation / stage / resource |36 /44 /22 pass; пересекаются с totals|
| Native8 |8/8, no regressions,0invalid/unobserved|
| LongMem Python discovery |55pass/5skip,60total|
| Protocol subprocess controls |5/5:4fault controls +1complete-capture control|
| Workspace clippy, all-targets, requiredfeatures, locked, -Dwarnings |exit0|
| Eval-support clippy / fmt / diff |exit0|

Строгий answer contract может отклонить незарегистрированную допустимую
перефразировку. Поэтому official scorer и independent exact results разделены;
при расхождении нужен casewise разбор, не выбор более высокого числа.

Cargo totals включают существующие architecture lints и doctests; ignored не
считаются passed. Финальные42 source/test hashes совпадают на build, tests и
captures. [Команды/totals](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/verification/topical-tabs-current/summary.json),
[release](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/verification/topical-tabs-current/release-build.json),
[native8](../../evals/longmemeval_v2/results/2026-11-26-candidate-work/native-gate-topical/comparison.json). Linux/Windows evals не запускались.

## Ранее закрытые четыре доработки

Эти измерения относятся к своим закреплённым execution identities; они не
выдаются за повторный model/extraction capture на новом Task7 binary.

- **Entity admission:** matched v2 corpus5/9→9/9; TP/FP/FN13/7/0→13/0/0;
  precision0,65→1, recall1→1, F1 0,787879→1. Разметка v2 не слепая; именованные
  Atlas,3M,Studio54,NASA и ручной resolve сохраняются. Доказаны скаляр/валюта и
  два английских предиката меры; другие языки/контексты и старая проекция не
  объявлены исправленными. [Evidence](../../evals/results/extraction-entity-repair-2026-11-26/README.md).
- **Closed evidence use:**25 public CLI controls для exact set, числа, source,
  abstention и точной quote. Неудачные raw-ID экраны6/12 и11/12 сохранены;
  v3 с handles для всех реально доставленных источников дал12/12+12/12.
  Это24/24 на раскрытых development-текстах, не доказательство generalization
  или изменения продуктового MCP-формата. [Evidence](../../evals/paired/memory-quality-repairs-2026-11-26/README.md).
- **Autonomous temporal reads:** две зарегистрированные реализации8/8+8/8,
  реальные выбранные агентом запросы/селекторы и оба-arm audits. Это конкретный
  lexical/regex workflow с явными временными осями; одинаковый продукт в arms,
  без agent-managed formation и без treatment-effect оценки.
  [Evidence](../../evals/paired/memory-quality-repairs-2026-11-26/selector-qualification-session-reuse/README.md).
- **Phase cancellation:** runtime RED→GREEN, реальные blocking reads506,266/
  506,265мс при500мс budget включая cleanup, сохранены successful/failed receipts.
  Этот предел не относится ко всей подготовке/formation.
  [Evidence](../../evals/paired/phase-deadlines-2026-11-26/).

## Выводы и конкретный action plan

Хорошо: происхождение и bi-temporal boundaries наблюдаемы; два исходных
full-store source failures исправлены без увеличения лимитов; повторные словари
устранены с независимым oracle; bounded snippets сохраняют реальные структурные
блоки. Отдельные value/source/refusal/citation критерии не позволяют подменить
правильный ответ наличием текста. Ошибки измерителя проверяются отдельно.

Плохо: ответная точность и false-premise handling остаются недостаточны; исходные
reviewed spans покрывают лишь часть вопросов; сохранение корректной альтернативы
не устраняет single-gold miss. Десятки секунд на read дороги. Обобщение entity
policy, source-citation UX и польза автономной памяти ещё не подтверждены.

| Приоритет | Что именно и как менять | Как принимать |
| --- | --- | --- |
| P1.0: runtime cost | Добавить attempt IDs/outcomes и отдельно измерять cold prefill, generation, retries. Для следующего протокола заранее выбрать per-call deadline и явный конечный retry budget; проверить отказ/cleanup на локальном controlled endpoint до моделей. | Число логических trials и requests раздельно; все timeouts retained. Cold start не прячется, warmup имеет отдельную стоимость; текущий frozen comparison не переписывать. |
| P1.1: reader / false premise / output | На фиксированных actual contexts отдельно сравнить source-first/final-answer-first протокол и исправленное экранирование; отдельным вариантом проверить читаемую проекцию serialized source с сохранёнными offsets/identity. Сохранить finish_reason и число HTTP attempts там, где доступны. Разделить malformed prompt, cap и использование evidence. | До calls заморозить одинаковые contexts/model/лимиты, closed labels, число captures/stop rule. Считать value, refusal, boxed, quote и errors отдельно. Старый raw256 baseline не переписывать, token cap не выдавать за memory fix. |
| P1.2: missing evidence | Для922a0c56 и exploratory32-source fetch сначала снять actual formation→fetch→admission→ranking→selection→packing; сопоставить exact source/альтернативы. После установленной границы — минимальный owning-context fix. | Independent public RED→GREEN и opposite controls; source binding, temporal/access/byte caps неизменны. Не поднимать limits вслепую, не promote gold IDs. Native8 и full primary pair должны сохраниться. |
| P1.3: durable access cost | Зафиксировать число/ID access updates при разных payload/index-key объёмах; если причина подтвердится, выделить лёгкую owning-context модель статистики. | Durability до return, duplicate ordering, decay, error/cancellation semantics. Real-store consistency + resource test; один registered full-store pair. Fire-and-forget неэквивалентен. |
| P2.1: entity generalization | Независимо разметить валюты, units, acronym/metric ambiguity, разные языки и genuine names/digits; отдельно source-linked audit старой automatic projection. | TP/FP/FN/linking по классам, precision без потерь genuine/manual. Без удаления фактов и ручных resolves; перестроение с fingerprint только по доказанной необходимости. |
| P2.2: agent citation UX | На независимых multi-source текстах сравнить raw IDs/handles при одинаковых evidence/order; затем решать о product response contract. | Чужие/неизвестные IDs и заимствованные quotes отклоняются; value/source/quote/refusal считаются отдельно. Исходный text-only LongMem adapter не измеряет citation quality. |
| P3: independent benefit | После квалификации целевого workflow зарегистрировать независимые истории, formation если входит в гипотезу, treatment/control, MDE/power, family-level paired unit и costs/stops. | До результатов frozen design; все failures в знаменателе. Старый C1 не возобновлять до удачного повтора. Пока C2 NO-GO. |

Closed tab-list repair остаётся в regression suite; general mixed-role intent
при необходимости должен получить отдельные source-backed fixtures до расширения
политики. Основной цикл разработки — маленький deterministic regression, затем
native8; дорогой full-store/model capture только после подтверждённой поправки.
Конечные наборы не дают100%-й гарантии общей корректности или пользы памяти.
