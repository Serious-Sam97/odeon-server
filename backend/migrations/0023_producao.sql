-- R22 — a ficha de produção.
--
-- ## A tabela que este arquivo não cria, e a coluna também não
--
-- O plano (`IDEIAS.md` §0 e §3) previa **colunas**: *"Um guia por região é um
-- `ALTER TABLE` mais uma revisita ao TMDB"*. Não é — e a razão é que o Odeon já
-- tem o mecanismo certo há muito tempo.
--
-- País e idioma são **tags**, no `tag`/`work_tag` do M2, exatamente como
-- `genre:` e `format:`. Isso não é economia de schema: é o que faz o eixo
-- funcionar sem escrever nada. O filtro por tag de `/api/works` existe desde o
-- M2, o guia (§30) já resolve gênero e década por ele, e uma coluna `pais`
-- exigiria um caminho de consulta novo pra responder a mesma pergunta que
-- `genre:Terror` já responde.
--
-- É a terceira vez seguida que a medição desfaz uma peça de schema prevista: o
-- `exemplar` da R19 (§35), o "plano B" da R21 (§37), e agora estas colunas.
--
-- ## O que esta migração faz, então
--
-- Só uma coisa: deixa `producao` entrar na lista de tipos de `job`. São 548
-- filmes com uma chamada externa cada — a dívida que o `IDEIAS.md` §8 registrou
-- (*"pela terceira vez um reparo de minutos vai correr dentro de um request"*),
-- e que aqui **não se repete**: nasce como `job`, com estado no banco,
-- progresso visível, cancelamento cooperativo e retomada pelo `WHERE`.
--
-- **Os oito valores anteriores continuam todos aqui.** Recriar o CHECK só com
-- os que alguém lembrasse apagaria os outros e faria o histórico de job
-- existente violar a própria constraint — a mesma nota que o 0013 deixou
-- escrita ao acrescentar `live_import` e o 0020 ao acrescentar `trivia`.

ALTER TABLE job DROP CONSTRAINT IF EXISTS job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check
    CHECK (kind IN ('scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
                    'live_import', 'trivia', 'producao'));
