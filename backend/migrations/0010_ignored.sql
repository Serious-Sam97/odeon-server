-- Estado terminal para o que NUNCA vai casar com provider nenhum.
--
-- Medido no acervo real: 1.234 dos 7.568 arquivos pendentes (16,3%) estavam
-- dentro de `Featurettes/`, `Extras/`, `Bonus/`, `Deleted Scenes/`,
-- `Webisodes/`. São making-of, cena deletada, promo — material que o TMDB não
-- cataloga. Eles ficavam eternamente na fila de revisão fingindo ser um
-- problema a resolver.
--
-- `ignored` não é "não identificado": é "não se aplica". A diferença importa
-- porque a fila mede trabalho pendente, e 16% dela era trabalho impossível.
--
-- É extensão de CHECK, que é exatamente o argumento do DESIGN §5 para ter
-- escolhido `text` + `CHECK` em vez de ENUM: um `ALTER TABLE` e acabou. Com
-- ENUM isto seria uma migração de tipo.
ALTER TABLE work DROP CONSTRAINT work_match_state_check;
ALTER TABLE work ADD CONSTRAINT work_match_state_check
    CHECK (match_state IN ('unmatched', 'auto', 'needs_review', 'confirmed', 'ignored'));

-- O índice parcial da fila não deve enxergar o que foi ignorado — senão a
-- contagem de pendências volta a incluí-los.
DROP INDEX IF EXISTS work_review_idx;
CREATE INDEX work_review_idx ON work (match_state, match_confidence DESC)
    WHERE match_state IN ('needs_review', 'unmatched');
