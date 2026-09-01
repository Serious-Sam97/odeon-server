-- Funde o gênero em inglês do AniList no vocabulário pt-BR que o acervo usa.
--
-- O `metadata::genero` conserta a **entrada**: daqui pra frente o AniList não
-- cria mais `Comedy` ao lado de `Comédia`. Esta migração conserta o que já
-- entrou — sem ela o painel de filtros continuaria com as duas listas, porque
-- ninguém vai rematchar 43 episódios à mão.
--
-- Medido em 17/08/2026, antes: `Comédia` 3.228 / `Comedy` 43, `Ação` 288 /
-- `Action` 43, `Aventura` 230 / `Adventure` 43, `Ficção científica` 160 /
-- `Sci-Fi` 43, `Sports` 43 (sem par). São **as mesmas 43 obras** nas cinco
-- tags: a temporada `anilist:288`, que não tem id do TMDB.
--
-- ⚠️ A lista é escrita por extenso, valor a valor, e não derivada de nenhuma
-- regra de semelhança. `Action & Adventure` (959 obras) e `Sci-Fi & Fantasy`
-- (1.240) são gêneros de série do TMDB, grupos próprios, e não a soma de
-- `Ação` com `Aventura` — qualquer casamento por palavra ou prefixo os
-- destruiria. Por extenso eles não têm como ser tocados.
--
-- São três comandos e não um só de propósito: uma CTE que escreve não enxerga
-- o que a outra escreveu — todas leem o mesmo instantâneo —, então o alvo
-- criado no passo 1 seria invisível pro passo 2 se estivessem juntos.

-- Tabela de trabalho: some no fim da migração, e evita repetir a lista.
CREATE TEMP TABLE de_para_genero(ingles text PRIMARY KEY, portugues text) ON COMMIT DROP;
INSERT INTO de_para_genero VALUES
    ('Action',        'Ação'),
    ('Adventure',     'Aventura'),
    ('Comedy',        'Comédia'),
    ('Sci-Fi',        'Ficção científica'),
    ('Sports',        'Esporte'),
    ('Horror',        'Terror'),
    ('Fantasy',       'Fantasia'),
    ('Mystery',       'Mistério'),
    ('Music',         'Música'),
    ('Psychological', 'Psicológico'),
    ('Slice of Life', 'Cotidiano'),
    ('Supernatural',  'Sobrenatural');

-- 1. O alvo pode não existir: `Esporte` não estava no acervo, porque não é um
--    gênero do TMDB. Só cria alvo de origem que realmente existe.
INSERT INTO tag (namespace, value)
SELECT 'genre', dp.portugues
FROM de_para_genero dp
WHERE EXISTS (SELECT 1 FROM tag o WHERE o.namespace = 'genre' AND o.value = dp.ingles)
ON CONFLICT (namespace, value) DO NOTHING;

-- 2. Repõe cada ligação no gênero em português. `DO NOTHING` porque nada
--    impede uma obra de já ter os dois — aqui não é o caso, mas a migração
--    não pode depender disso.
INSERT INTO work_tag (work_id, tag_id, weight, source)
SELECT wt.work_id, destino.id, wt.weight, wt.source
FROM de_para_genero dp
JOIN tag origem   ON origem.namespace  = 'genre' AND origem.value  = dp.ingles
JOIN tag destino  ON destino.namespace = 'genre' AND destino.value = dp.portugues
JOIN work_tag wt  ON wt.tag_id = origem.id
ON CONFLICT (work_id, tag_id) DO NOTHING;

-- 3. A tag em inglês sai, e o `ON DELETE CASCADE` do `work_tag` leva as
--    ligações antigas junto.
DELETE FROM tag
WHERE namespace = 'genre'
  AND value IN (SELECT ingles FROM de_para_genero);
