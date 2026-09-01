-- A R75 rodou contra um banco que ainda não tinha a biblioteca — e por isso não
-- fez nada.
--
-- ## O que foi medido
--
-- 30/08/2026, `/api/works` com o `tags_not=format:série,format:anime` que a aba
-- de filmes manda:
--
--     Youtube   2.180
--     Movies      965
--     ──────────────
--     total     3.145      dos quais 69% não são filme
--
-- E os outros 331 arquivos do YouTube — os numerados, `001 - Payday 2`, os dos
-- Irmãos Piologo — estão em `format:série`, na prateleira errada e fora do
-- canal. 2.180 + 331 = 2.511, a biblioteca inteira.
--
-- ## Por que as 0048/0049/0051 não pegaram
--
-- Elas rodaram. `_sqlx_migrations` diz `success = t` para as quatro. O relógio
-- diz o resto:
--
--     16:50:33   0048 — UPDATE library SET default_kind = 'video'
--                       WHERE provider_hint = 'none' AND default_kind = 'other'
--     17:04:03   biblioteca "Movies"  criada
--     17:50:12   biblioteca "Youtube" criada, com default_kind = 'other'
--
-- A 0048 procurou a biblioteca uma hora antes de ela existir, casou com zero
-- linhas, e foi marcada como aplicada. A 0049 e a 0051 dependem do que a 0048
-- deixaria escrito (`w.kind = 'video'`, `l.default_kind = 'video'`), então
-- casaram com zero também.
--
-- **Isso não é acidente desta instalação, é a ordem normal de qualquer uma:**
-- migração roda no boot, biblioteca se cria depois. Num banco novo a R75 nunca
-- ia pegar, porque o dado que ela conserta ainda não nasceu.
--
-- ## Por que esta não vai repetir o erro
--
-- Ela não vai. Uma migração conserta o que está no banco **hoje**, e é só isso
-- que ela sabe fazer — a próxima biblioteca criada amanhã está fora do alcance
-- de qualquer `UPDATE` escrito agora, exatamente como esta estava fora do
-- alcance da 0048.
--
-- O que impede a repetição está no `create_library` e no `update_library`, que
-- passaram a corrigir a combinação `provider_hint = 'none'` com `default_kind`
-- que não seja `video`. A regra morava só aqui dentro; agora mora no caminho
-- que cria a linha, e a migração voltou a ser o que devia ser: o retroativo.

-- 1. A biblioteca declara o que ela é. Sem isto o `Guess::kind` continua caindo
--    no `else` e gravando `other` em arquivo novo — a guarda que a R75 pôs lá
--    (`if library_default == "video"`) está certa e nunca dispara.
UPDATE library SET default_kind = 'video'
 WHERE provider_hint = 'none' AND default_kind <> 'video';

-- 2. O que já está no banco acompanha: `other` (o "não sei" do scanner) e
--    `episode` (o número na frente do nome, que numa biblioteca de canal não
--    quer dizer episódio). `music_video` fica fora de novo — a biblioteca de
--    clipes é outra coisa, e continua sendo.
UPDATE work w SET kind = 'video', updated_at = now()
 WHERE w.kind IN ('other', 'episode')
   AND EXISTS (SELECT 1 FROM media_file mf JOIN library l ON l.id = mf.library_id
                WHERE mf.work_id = w.id AND l.default_kind = 'video');

-- 3. Sai o formato errado, entra o certo. O DELETE é o que tira os 331 da
--    prateleira `série`; sem ele a obra ficaria com as duas etiquetas e
--    apareceria em duas prateleiras que a tela apresenta como excludentes.
DELETE FROM work_tag wt
 USING tag t, work w
 WHERE t.id = wt.tag_id AND w.id = wt.work_id
   AND t.namespace = 'format' AND t.value <> 'vídeo'
   AND w.kind = 'video';

INSERT INTO tag (namespace, value) VALUES ('format', 'vídeo')
ON CONFLICT (namespace, value) DO NOTHING;

INSERT INTO work_tag (work_id, tag_id, source)
SELECT w.id, t.id, 'inferred'
FROM work w JOIN tag t ON t.namespace = 'format' AND t.value = 'vídeo'
WHERE w.kind = 'video' AND w.match_state <> 'ignored'
ON CONFLICT (work_id, tag_id) DO NOTHING;
