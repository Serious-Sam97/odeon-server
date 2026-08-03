-- R33 — a rede social: post, comentário e mensagem.
--
-- ## Três tabelas, e é a primeira fase em muito tempo que precisa mesmo delas
--
-- As cinco fases anteriores previram peça de schema e não criaram nenhuma (§38,
-- §41, §48). Esta cria três, e a diferença é simples: feed, XP e conquista são
-- **leituras** de fatos que já existiam; post, comentário e mensagem são fatos
-- novos. Ninguém deriva um texto que uma pessoa escreveu.
--
-- ## O comentário serve os dois alvos, e é uma tabela só
--
-- Decidido: comentário existe **no post e na review**. Post sem comentário é
-- diário, não rede social; e a review foi pedida com *"as pessoas podem
-- comentar"* explícito (`IDEIAS.md` §3.4).
--
-- Duas tabelas quase idênticas seriam duas telas, duas rotas e duas chances de
-- divergirem sobre o que é um comentário. Uma tabela com **alvo polimórfico**,
-- com CHECK garantindo exatamente um, é o mesmo padrão que `emprestimo` usa
-- desde a 0021 pra apontar ou pra uma obra ou pra uma coleção — e pelo mesmo
-- motivo: as duas pontas mantêm chave estrangeira de verdade, o que um
-- `alvo_id text` genérico perderia.

-- --------------------------------------------------------------------- o post

CREATE TABLE post (
    id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- O texto. O limite é de tela: acima disso o feed vira blog, e a régua é a
    -- mesma que a resenha usa (`MAX_TEXTO` em `avaliacao.rs` são 2000 pra uma
    -- ficha inteira). Um post é uma frase no corredor, não um ensaio.
    texto text NOT NULL CHECK (length(btrim(texto)) BETWEEN 1 AND 500),

    -- **A obra citada, quando há.** É o que faz o post ser sobre alguma coisa em
    -- vez de sobre nada: *"esse final me pegou"* com a capa do filme ao lado é
    -- outra coisa que a mesma frase solta.
    --
    -- `ON DELETE SET NULL` e não CASCADE: apagar uma obra do acervo não deve
    -- apagar o que alguém escreveu. Some a capa, fica o texto.
    work_id uuid REFERENCES work(id) ON DELETE SET NULL,

    criado_em timestamptz NOT NULL DEFAULT now()
);

-- "Os posts destas pessoas, os novos primeiro" — a pergunta do feed, feita uma
-- vez por carregamento.
CREATE INDEX post_recente_idx ON post (user_id, criado_em DESC);
-- E "os posts sobre esta obra", que a ficha do filme faz.
CREATE INDEX post_obra_idx ON post (work_id) WHERE work_id IS NOT NULL;

-- ---------------------------------------------------------------- o comentário

CREATE TABLE comentario (
    id      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    texto   text NOT NULL CHECK (length(btrim(texto)) BETWEEN 1 AND 500),

    -- O alvo: **ou** um post, **ou** uma review. Nunca os dois, nunca nenhum.
    post_id uuid REFERENCES post(id) ON DELETE CASCADE,

    -- A review é identificada pelo par que é a chave primária de `avaliacao`.
    -- Duas colunas em vez de um id porque `avaliacao` não tem id próprio — a
    -- chave dela **é** (quem, qual filme), e inventar um id só pra ser apontado
    -- aqui trocaria a identidade natural por uma sintética.
    review_user uuid,
    review_work uuid,
    FOREIGN KEY (review_user, review_work)
        REFERENCES avaliacao(user_id, work_id) ON DELETE CASCADE,

    -- Exatamente um alvo. Sem isto, "comentário sem dono" e "comentário nos dois
    -- lugares" são estados alcançáveis, e os dois aparecem como linha fantasma
    -- em alguma tela.
    CONSTRAINT comentario_um_alvo CHECK (
        (post_id IS NOT NULL)::int
      + (review_user IS NOT NULL AND review_work IS NOT NULL)::int = 1
    ),
    -- E as duas colunas da review andam juntas: meia review é meio alvo.
    CONSTRAINT comentario_review_completa CHECK (
        (review_user IS NULL) = (review_work IS NULL)
    ),

    criado_em timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX comentario_post_idx ON comentario (post_id, criado_em)
    WHERE post_id IS NOT NULL;
CREATE INDEX comentario_review_idx ON comentario (review_user, review_work, criado_em)
    WHERE review_user IS NOT NULL;

COMMENT ON TABLE comentario IS
    'Comentário de post OU de review. O CHECK garante exatamente um alvo.';

-- ----------------------------------------------------------------- a mensagem

-- Direta, entre duas pessoas. **Não é o par canônico da amizade** (0028): ali a
-- linha descreve uma relação simétrica, e aqui ela descreve um ato com direção —
-- quem mandou e quem recebeu não são intercambiáveis.
CREATE TABLE mensagem (
    id    bigserial PRIMARY KEY,
    de    uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    para  uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    texto text NOT NULL CHECK (length(btrim(texto)) BETWEEN 1 AND 2000),

    criado_em timestamptz NOT NULL DEFAULT now(),
    -- Quando quem recebeu abriu a conversa. Nulo é não lida, e é o que acende o
    -- contador na aba.
    lido_em   timestamptz,

    CONSTRAINT mensagem_nao_e_monologo CHECK (de <> para)
);

-- A conversa entre duas pessoas, em ordem. O índice cobre os dois sentidos
-- porque uma conversa é lida inteira, não por remetente.
CREATE INDEX mensagem_conversa_idx ON mensagem (de, para, criado_em DESC);
CREATE INDEX mensagem_recebida_idx ON mensagem (para, criado_em DESC);
-- "Quantas eu não li" — a pergunta que a aba faz a cada carregamento.
CREATE INDEX mensagem_nao_lida_idx ON mensagem (para) WHERE lido_em IS NULL;
