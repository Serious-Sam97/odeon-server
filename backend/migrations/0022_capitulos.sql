-- R21 — capítulos, pro menu de DVD.
--
-- ## O que foi medido antes de escrever isto
--
-- O `IDEIAS.md` §3 exigia medir a cobertura antes de desenhar a tela de
-- capítulos, e a medição foi mais dura que o esperado. Nos **548 filmes
-- identificados** deste acervo:
--
-- | | |
-- |---|---|
-- | com capítulos | **74 (13,5%)** |
-- | com **nomes** de capítulo úteis | **9 (1,6%)** |
-- | mediana de capítulos, quando há | 16 |
--
-- Os "nomes" dos outros são vazios, `Chapter 01`, ou — pior e mais comum —
-- **o próprio timecode** repetido no campo de título. Um menu de capítulos
-- construído sobre nomes está morto: ele funcionaria em nove filmes.
--
-- Daí a decisão que reorganiza a tela: **a grade de cenas é o principal, e o
-- capítulo é só uma âncora melhor quando existe.** Não é degradação — é o que
-- o "scene selection" de um DVD sempre foi: uma grade de miniaturas com
-- timecode. Nome de capítulo era raro até nos discos prensados.
--
-- ## Por que uma coluna, e não uma passada nova pelo acervo
--
-- `ffprobe -show_chapters` custa **242 ms** por arquivo — barato, mas não de
-- graça a cada abertura de menu. Esta coluna é cache, exatamente como
-- `work_trivia` (§33): lê-se sob demanda na primeira abertura e guarda-se.
--
-- E **NULL é diferente de `[]`**: `NULL` é "nunca perguntei", `[]` é
-- "perguntei e este arquivo não tem" — que é a resposta de 86,5% deles. Sem a
-- distinção, os 474 filmes sem capítulo seriam reprobados para sempre. É a
-- mesma sutileza que a §33 registrou pra trivia vazia.
--
-- Não há reprobe: o `probe` do scanner continua sem `-show_chapters`. Passar
-- 17.498 arquivos de novo pra preencher uma coluna que só interessa a 548
-- filmes seria pagar caro pelo lugar errado.

ALTER TABLE media_file ADD COLUMN chapters jsonb;

COMMENT ON COLUMN media_file.chapters IS
    'Capítulos lidos do container. NULL = nunca lido; [] = lido e não tem.';

-- "O que ainda não foi perguntado" é a pergunta de um futuro aquecimento, e
-- ela só interessa a filme.
CREATE INDEX media_file_sem_capitulos_idx ON media_file (work_id)
    WHERE chapters IS NULL;
