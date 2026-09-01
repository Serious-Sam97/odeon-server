-- O formato deixa de depender da identificação (R64).
--
-- Medido em 18/08/2026, nos contadores agrupados de `/api/library`:
-- `format:série` 120, `format:filme` 834, `format:anime` 3 — **957 de 8.333
-- entradas**. As outras ~7.376 sumiam de qualquer prateleira e só existiam em
-- "tudo", porque a etiqueta era escrita no `apply_candidate` e nada mais.
--
-- O `metadata::formato` passa a escrevê-la no scanner, a partir do `kind`. Esta
-- migração faz o mesmo com o que já está no banco: é a diferença entre
-- consertar o defeito e consertá-lo daqui pra frente.
--
-- Por `kind`, o que entra:
--
--     episode      → série
--     movie        → filme
--     music_video  → clipe
--     other        → nada
--
-- ⚠️ `other` fica de fora, e é a única parte disto que é decisão e não conta.
-- Ele é o `kind` que o scanner usa quando **ele mesmo** não sabe o que o
-- arquivo é; escrever `filme` ali trocaria uma ausência honesta por um palpite
-- com cara de dado. São 2.182 entradas que continuam só em "tudo".
--
-- ⚠️ E `NOT EXISTS (… namespace = 'format')` é o que impede esta migração de
-- desfazer identificação: um episódio de anime já tem `format:anime`, escrito
-- porque o provider foi o AniList, e o `kind` dele é `episode` — sem a guarda,
-- ele viraria `série` e perderia a única informação que o arquivo não dá.

INSERT INTO tag (namespace, value)
VALUES ('format', 'série'), ('format', 'filme'), ('format', 'clipe')
ON CONFLICT (namespace, value) DO NOTHING;

INSERT INTO work_tag (work_id, tag_id, source)
SELECT w.id, t.id, 'inferred'
FROM work w
JOIN tag t ON t.namespace = 'format'
          AND t.value = CASE w.kind
                            WHEN 'episode'     THEN 'série'
                            WHEN 'movie'       THEN 'filme'
                            WHEN 'music_video' THEN 'clipe'
                        END
WHERE w.match_state <> 'ignored'
  AND NOT EXISTS (
      SELECT 1 FROM work_tag wt JOIN tag t2 ON t2.id = wt.tag_id
      WHERE wt.work_id = w.id AND t2.namespace = 'format'
  )
ON CONFLICT (work_id, tag_id) DO NOTHING;
