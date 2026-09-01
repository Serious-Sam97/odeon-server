-- O job da estrutura (R75) precisa de um `kind` próprio na allowlist.
ALTER TABLE job DROP CONSTRAINT job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check CHECK (kind IN (
    'scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
    'live_import', 'trivia', 'producao', 'saga', 'temporada', 'estrutura'
));
