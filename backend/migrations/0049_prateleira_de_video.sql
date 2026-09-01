-- A prateleira dos vídeos de canal — R75, segunda metade.
--
-- A 0048 trocou o `kind` de 2.182 obras de `other` pra `video`; esta dá a elas
-- a etiqueta de formato, que é o que as tira de "tudo" e as põe numa prateleira
-- própria. Mesma forma da 0046, e a mesma guarda: `NOT EXISTS` impede que ela
-- desfaça formato já escrito.
INSERT INTO tag (namespace, value) VALUES ('format', 'vídeo')
ON CONFLICT (namespace, value) DO NOTHING;

INSERT INTO work_tag (work_id, tag_id, source)
SELECT w.id, t.id, 'inferred'
FROM work w
JOIN tag t ON t.namespace = 'format' AND t.value = 'vídeo'
WHERE w.kind = 'video'
  AND w.match_state <> 'ignored'
  AND NOT EXISTS (
      SELECT 1 FROM work_tag wt JOIN tag t2 ON t2.id = wt.tag_id
      WHERE wt.work_id = w.id AND t2.namespace = 'format')
ON CONFLICT (work_id, tag_id) DO NOTHING;
