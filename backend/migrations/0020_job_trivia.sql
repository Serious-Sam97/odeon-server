-- `trivia` entra na lista de tipos de job.
--
-- O aquecimento do cache de trivia (§32) é operação longa — 548 filmes, duas
-- chamadas externas cada — e por isso nasce como `job`, e não como requisição
-- síncrona (§12, e a dívida que o §21 registrou).
--
-- Só que `job.kind` tem um CHECK com a lista dos tipos, e sem esta migração o
-- INSERT era recusado pelo banco. O sintoma foi pior que o erro: `Job::start`
-- trata falha e "já existe um ativo" da mesma forma — devolve `None` — então a
-- rota respondia *"já há um aquecimento em andamento"* quando nunca houvera
-- nenhum. Um erro que se disfarça de estado normal.
--
-- **Os sete valores anteriores continuam todos aqui.** Recriar o CHECK só com
-- os que eu lembrasse apagaria os outros e faria o histórico de job existente
-- violar a própria constraint — a mesma nota que o 0013 já tinha deixado
-- escrita ao acrescentar `live_import`.
--
-- Isto é, literalmente, o argumento do §5 para `text` + `CHECK` em vez de ENUM:
-- acrescentar um valor é um `ALTER TABLE`, não uma migração dolorosa.

ALTER TABLE job DROP CONSTRAINT IF EXISTS job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check
    CHECK (kind IN ('scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
                    'live_import', 'trivia'));
