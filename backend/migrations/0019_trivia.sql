-- Curiosidades sobre o FILME, vindas de fora.
--
-- As curiosidades da §31 são derivadas do próprio acervo ("de Martin Campbell
-- você também tem…"). São boas e são únicas deste servidor, mas não é isso que
-- alguém quer dizer com "curiosidade sobre o filme": aquilo é sobre a sua
-- estante, não sobre a obra.
--
-- Trivia de verdade precisa de fonte, e o §31 já tinha descartado três. A
-- quarta é a que faltava: **o Wikidata**, que casa pelo id do TMDB
-- (propriedade P4947) — exato, sem adivinhar título, do mesmo jeito que o
-- `provider_key` do §8h dedupe pessoa. Medido em 12 filmes sorteados do
-- acervo: **12 de 12 casaram**.
--
-- Esta tabela é cache, não fonte. Ela existe por três razões:
--
--  1. **A ficha não pode depender da rede.** Sem cache, abrir uma obra faria
--     duas chamadas externas e a seção demoraria segundos — toda vez.
--  2. **Educação com o serviço alheio.** O endpoint SPARQL do Wikidata é
--     público e gratuito; buscar o mesmo filme a cada abertura seria abuso.
--  3. **A biblioteca continua funcionando offline**, que é a mesma razão pela
--     qual artwork e retratos ficam em disco desde o M1.
--
-- `buscado_em` permite reconsultar o que envelheceu sem precisar de coluna de
-- controle: a política de validade fica no código, não no schema.
--
-- Linha com `itens = '[]'` é resposta legítima e **é guardada de propósito**:
-- significa "procurei e não há", e sem ela todo filme sem trivia seria
-- reconsultado para sempre.

CREATE TABLE work_trivia (
    work_id     uuid PRIMARY KEY REFERENCES work(id) ON DELETE CASCADE,
    itens       jsonb NOT NULL DEFAULT '[]'::jsonb,
    buscado_em  timestamptz NOT NULL DEFAULT now()
);

-- "O que ainda não foi buscado" e "o que está velho" são a mesma pergunta para
-- um futuro job de backfill.
CREATE INDEX work_trivia_buscado_idx ON work_trivia (buscado_em);
