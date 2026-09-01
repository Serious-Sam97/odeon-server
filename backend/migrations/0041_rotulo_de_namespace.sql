-- O painel de filtros escrevia `COUNTRY` no meio de `FORMATO` e `GÊNERO`.
--
-- A causa não é o `country`: é que a semeadura do 0003 nomeou os namespaces que
-- existiam **naquele dia** (`format`, `genre`, `mood`, `vibe`, `origin`) e a
-- ingestão criou dois novos depois — `country` e `lang`, do M4 — sem passar por
-- aqui. Três dos cinco nomeados não têm uma tag sequer; os dois que faltam
-- somam 1.911 usos.
--
-- Por isso o conserto é em duas partes: esta migração nomeia os dois que já
-- existem, e o `list_namespaces` passa a devolver **todo** namespace que a
-- tabela `tag` contém, com um rótulo de queda pra quem chegar amanhã. Só a
-- migração deixaria o defeito armado pro próximo namespace.
--
-- Posição 60/70 põe os dois depois dos cinco antigos, que ficam em 10..50.
INSERT INTO tag_namespace (namespace, label, color, position) VALUES
    ('country', 'País',   '#f7768e', 60),
    ('lang',    'Idioma', '#73daca', 70)
ON CONFLICT (namespace) DO UPDATE SET
    label = EXCLUDED.label,
    color = COALESCE(tag_namespace.color, EXCLUDED.color);
