-- O job que busca a ficha das temporadas (R63) precisa de um `kind` próprio.
--
-- O `job.kind` é uma allowlist desde a migração original, e é ela que faz o
-- `Job::start` recusar um nome errado em vez de gravar lixo — o mesmo motivo
-- pelo qual `saga` e `producao` passaram por aqui antes.
ALTER TABLE job DROP CONSTRAINT job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check CHECK (kind IN (
    'scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
    'live_import', 'trivia', 'producao', 'saga', 'temporada'
));
