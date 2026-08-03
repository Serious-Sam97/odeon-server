-- R35 — os desafios.
--
-- O único item das onze anotações que nunca tinha sido construído.
--
-- > *"Desafios: tarefas com prazo, que dão experiência. Mais simples que os
-- > temas do guia, e sorteadas para cada pessoa — não são iguais pra todos. A
-- > cadência é escolhida pela pessoa, entre algumas opções definidas."*
--
-- ## Por que isto precisa de tabela, e o guia não precisou
--
-- O tema da semana (§50) é derivado: `md5(semana || eixo)`, igual pra todo
-- mundo, recalculável a qualquer momento. O desafio **não é**, por duas razões:
--
--  * ele é sorteado **por pessoa**, e a cadência é escolhida por ela — então a
--    janela de cada um começa e termina em instantes diferentes, e não há um
--    "agora" comum de onde derivar;
--  * e **cumprir é um fato dentro de uma janela que fecha**. Depois que ela
--    passa, "terminou um filme de terror enquanto o desafio estava de pé" deixa
--    de ser recuperável — a mesma razão da `evento_participacao` (§50) e do
--    `emprestimo.devolvido_como` (§35).
--
-- É o §2.4 do `IDEIAS.md` no schema: o guia é coletivo e não guarda nada; o
-- desafio é individual e guarda.

CREATE TABLE desafio (
    id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- A definição, em `src/desafios.rs`. Texto e não enum pelo mesmo motivo das
    -- conquistas: a lista cresce, e um CHECK que precisa ser alterado a cada
    -- desafio novo é um CHECK que alguém vai esquecer.
    chave text NOT NULL,

    -- O parâmetro sorteado: "Terror", "Japão", "1980". `NULL` nos desafios que
    -- não têm alvo ("termine qualquer obra").
    alvo text,

    -- Quanto vale. **Guardado na linha**, e não lido da definição na hora de
    -- somar: mudar o valor de um desafio amanhã não deve reescrever o XP de
    -- quem já cumpriu ontem. É o mesmo princípio de congelar
    -- `emprestimo.devolvido_como`.
    xp int NOT NULL CHECK (xp > 0),

    comeca_em timestamptz NOT NULL,
    vence_em  timestamptz NOT NULL,
    CONSTRAINT desafio_janela_valida CHECK (vence_em > comeca_em),

    cumprido_em   timestamptz,
    -- Qual obra fechou. Nulo nos que não são sobre assistir (avaliar, alugar).
    cumprido_work uuid REFERENCES work(id) ON DELETE SET NULL,

    -- Um desafio por chave por janela por pessoa. É o que torna a geração
    -- **idempotente**: abrir a tela duas vezes na mesma janela não sorteia
    -- dois conjuntos, porque o segundo `INSERT` não passa.
    UNIQUE (user_id, comeca_em, chave)
);

-- "Os meus desta janela" — a pergunta da tela, feita a cada carregamento.
CREATE INDEX desafio_janela_idx ON desafio (user_id, comeca_em DESC);
-- "Os meus em aberto agora" — a pergunta que roda a cada progresso gravado, e
-- que portanto precisa ser barata.
CREATE INDEX desafio_aberto_idx ON desafio (user_id) WHERE cumprido_em IS NULL;

COMMENT ON TABLE desafio IS
    'Tarefas com prazo, sorteadas por pessoa. Falhar não tem consequência: a janela fecha e outro é sorteado.';

-- ---------------------------------------------------------------- a cadência

-- Escolhida pela pessoa, entre opções definidas em `src/desafios.rs`.
--
-- Mora no `perfil` porque é preferência de conta, e o perfil já é onde a pessoa
-- decide como ela aparece. Uma tabela só pra isto seria uma linha por usuário
-- pra guardar uma palavra.
--
-- **Sem CHECK de valor**, pelo mesmo motivo do `perfil.titulo`: quem conhece a
-- lista de opções é o código, e um CHECK aqui teria que ser alterado toda vez
-- que uma cadência nova entrasse.
ALTER TABLE perfil
    ADD COLUMN cadencia text NOT NULL DEFAULT 'semanal';

COMMENT ON COLUMN perfil.cadencia IS
    'Cadência dos desafios: diaria | tres_dias | semanal. Validada em src/desafios.rs.';
