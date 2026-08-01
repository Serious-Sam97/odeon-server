-- Correção humana do PARSE, persistida.
--
-- POR QUE ESTA COLUNA EXISTE
-- A busca manual (`POST /api/works/{id}/search`) deixava o usuário digitar o
-- título certo, mas a correção morria no handler: ela mutava um `Guess` local
-- só pra montar a consulta ao provider e era descartada no retorno. Na hora de
-- confirmar, o `load_work_context` re-derivava tudo do CAMINHO de novo — então
-- escolher a série certa para `Frieren - 37.mkv` ainda resultava numa busca por
-- temporada 1, episódio 37, e num título "Episódio 37".
--
-- Guardando o override, a correção sobrevive ao confirm, ao re-scan e ao
-- re-match. É decisão humana: o `reset` a preserva, porque desfazer a
-- identificação não desfaz o que a pessoa ensinou sobre o arquivo.
--
-- Formato: só os campos corrigidos, mesclados por cima do parse do caminho.
--   {"title": "Frieren", "season": 1, "episode": 37, "kind": "episode"}
ALTER TABLE work ADD COLUMN parse_override jsonb;

-- Séries e temporadas criadas pelo MATCHER estavam marcadas como 'manual'.
--
-- A coluna nasce com default 'manual' e o `ensure_collection` não a preenchia.
-- Consequência prática: o `reset` simétrico não teria como distinguir a
-- coleção que o provider trouxe da playlist que alguém montou à mão — e
-- apagaria a errada, ou nenhuma.
--
-- `provider_key IS NOT NULL` é o sinal confiável: só o matcher preenche.
UPDATE collection SET origin = 'provider'
WHERE provider_key IS NOT NULL AND origin <> 'provider';
