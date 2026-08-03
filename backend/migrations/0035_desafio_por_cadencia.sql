-- R35b — a cadência faz parte da identidade da janela.
--
-- ## O defeito, e ele só aparece numa segunda-feira
--
-- Todas as cadências são ancoradas na **segunda-feira local** — a mesma âncora
-- da vitrine (§36) e do guia (§50), e ela está certa: sem ela, a janela de cada
-- pessoa flutuaria a partir do dia em que ela escolheu a cadência.
--
-- Mas numa segunda-feira a janela diária e a semanal **começam no mesmo
-- instante**. E como a identidade da linha era `(user_id, comeca_em, chave)`,
-- trocar de cadência naquele dia não gerava nada: o `ON CONFLICT DO NOTHING`
-- via as linhas da semana anterior e desistia.
--
-- O sintoma é discreto e por isso pior: os desafios apareciam, a cadência dizia
-- "todo dia", e o prazo continuava sendo o de domingo. Ninguém veria isso como
-- defeito — veria como o produto ignorando a escolha.
--
-- ## O conserto
--
-- A cadência entra na chave. Duas janelas que começam no mesmo instante mas
-- duram tempos diferentes **são duas janelas**, e é isso que a chave passa a
-- dizer.
--
-- Encontrado testando: trocar de cadência numa segunda e olhar o prazo.

ALTER TABLE desafio
    ADD COLUMN cadencia text NOT NULL DEFAULT 'semanal';

-- A antiga não distingue as duas janelas do mesmo começo.
ALTER TABLE desafio DROP CONSTRAINT desafio_user_id_comeca_em_chave_key;

ALTER TABLE desafio
    ADD CONSTRAINT desafio_janela_unica UNIQUE (user_id, comeca_em, cadencia, chave);

COMMENT ON COLUMN desafio.cadencia IS
    'Faz parte da identidade da janela: numa segunda-feira, a diária e a semanal começam no mesmo instante.';
