-- Vídeo de canal com número na frente continua sendo vídeo — R75.
--
-- O `Guess::kind` dizia "tem número de episódio → é episódio", e isso vale numa
-- biblioteca de série. Numa de canal, não: `001 - Payday 2` é vídeo, e virava
-- `episode` só pela numeração. Medido em 20/08/2026: **329 obras** da
-- biblioteca do YouTube estavam como episódio — os 331 dos Irmãos Piologo entre
-- elas. Apareciam na prateleira `série` e ficavam **fora do canal**, que agrupa
-- `kind = 'video'`.
--
-- O número não se perde: `season_number` e `episode_number` continuam onde
-- estão, e são o que ordena o vídeo dentro da playlist.
UPDATE work w SET kind = 'video', updated_at = now()
 WHERE w.kind = 'episode'
   AND EXISTS (SELECT 1 FROM media_file mf JOIN library l ON l.id = mf.library_id
                WHERE mf.work_id = w.id AND l.default_kind = 'video');

-- E a prateleira acompanha: sai `série`, entra `vídeo`.
DELETE FROM work_tag wt
 USING tag t, work w
 WHERE t.id = wt.tag_id AND w.id = wt.work_id
   AND t.namespace = 'format' AND t.value <> 'vídeo'
   AND w.kind = 'video';

INSERT INTO work_tag (work_id, tag_id, source)
SELECT w.id, t.id, 'inferred'
FROM work w JOIN tag t ON t.namespace = 'format' AND t.value = 'vídeo'
WHERE w.kind = 'video' AND w.match_state <> 'ignored'
ON CONFLICT (work_id, tag_id) DO NOTHING;
