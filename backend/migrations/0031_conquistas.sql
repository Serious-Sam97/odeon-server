-- R32 — conquistas, nível e perfil.
--
-- ## O que esta migração NÃO cria
--
-- **A lista de conquistas.** Ela mora no código (`src/conquistas.rs`), e a
-- decisão é de quem decide: *"quem escreve a lista é quem programa"*. Uma tabela
-- de definições daria uma tela de administração pra criar conquista — que é
-- exatamente o que ninguém pediu — e faria a regra de desbloqueio virar dado,
-- quando ela é código: "terminou 10 filmes de terror" é um `SELECT`, não uma
-- linha.
--
-- **E o XP.** Ele é **derivado**, não acumulado. Não há tabela de pontos, não há
-- ledger, não há job de recálculo — o nível de alguém é uma função do que essa
-- pessoa fez, lida na hora. Isso rende três coisas:
--
--  * **as conquistas são retroativas de graça.** Não há backfill: no dia em que
--    isto ligar, quem já terminou dois filmes já terminou dois filmes. É a
--    decisão do `IDEIAS.md` §3.3, e ela custou zero linhas;
--  * nada desincroniza. Um contador que soma a cada evento erra pra sempre no
--    dia em que um evento se perde;
--  * apagar um empréstimo ou uma avaliação corrige o XP sozinho, em vez de
--    deixar pontos órfãos de um fato que não existe mais.
--
-- É a quinta vez seguida em que a peça de schema prevista não nasce — o §38
-- registrou as três primeiras, o §41 a quarta.
--
-- ## E a saga também não nasce
--
-- Conquista de trilogia precisa que o Odeon saiba o que é uma saga, e o
-- `IDEIAS.md` §7 registra isso como dívida: *"`belongs_to_collection` do TMDB
-- não é buscado"*. A dívida é de **dados**, não de schema: `collection.kind`
-- aceita `'franchise'` desde a migração original.
--
-- O modelo de grafo do §1 tinha previsto a saga antes de alguém precisar dela, e
-- é a segunda vez que ele paga essa aposta (a primeira foi a ordem alternativa
-- de exibição). O que falta é o job que preenche — `metadata/saga.rs`.

-- ---------------------------------------------------------- o desbloqueio

-- **Só o fato, e quando.** Que conquista existe, o que ela exige e quanto vale
-- é do código; o que o banco guarda é o instante em que ela deixou de estar
-- trancada pra alguém.
--
-- Guardar o instante é o que permite dizer *"você desbloqueou isto ontem"* e o
-- que impede a mesma conquista de piscar como nova a cada leitura. Sem a linha,
-- o avaliador não teria como distinguir "acabou de acontecer" de "sempre esteve
-- lá" — e a diferença entre as duas é a única coisa que uma conquista tem.
CREATE TABLE conquista_do_usuario (
    user_id uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- A chave da definição em `conquistas.rs`. **Texto e não enum**: o CHECK
    -- teria que ser alterado a cada conquista nova, e a lista é pra ser longa.
    -- Uma chave órfã (definição removida) some da tela sozinha, porque quem
    -- monta a lista é o código.
    chave text NOT NULL,

    -- Quando foi observada pela primeira vez.
    --
    -- **Não é quando o feito aconteceu**, e a distinção importa: as conquistas
    -- são retroativas, então no dia em que isto ligar tudo que já era verdade
    -- desbloqueia de uma vez, com o carimbo de hoje. Fingir a data do feito
    -- exigiria reconstruir o instante exato de cada regra a partir do
    -- histórico — e para "assistiu 10 filmes de terror" esse instante é uma
    -- consulta que ninguém quer manter.
    em timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (user_id, chave)
);

-- "As conquistas desta pessoa, as novas primeiro" — a pergunta do perfil.
CREATE INDEX conquista_recente_idx ON conquista_do_usuario (user_id, em DESC);

COMMENT ON TABLE conquista_do_usuario IS
    'Só o desbloqueio. A definição, a regra e os pontos moram em src/conquistas.rs.';

-- ------------------------------------------------------------------ o perfil

-- Uma linha por pessoa, criada na primeira vez que alguém salva alguma coisa.
-- Perfil vazio é a ausência da linha, e não uma linha de campos nulos: quem
-- nunca mexeu não tem perfil, tem conta.
CREATE TABLE perfil (
    user_id uuid PRIMARY KEY REFERENCES app_user(id) ON DELETE CASCADE,

    -- O título em exibição. **É uma chave de conquista**, não texto livre — e é
    -- por isso que ele não tem CHECK aqui: quem valida se a pessoa desbloqueou
    -- aquele título é o código, que é quem conhece a lista.
    --
    -- Um título que a pessoa não conquistou é a única mentira que este perfil
    -- poderia contar, e a validação mora onde a verdade mora.
    titulo text,

    -- As tags escolhidas entre as desbloqueadas. Mesma regra do título.
    --
    -- O limite de cinco é de tela, não de banco: uma fileira de quinze etiquetas
    -- deixa de ser identidade e vira nuvem de palavras.
    tags text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 5),

    -- E o campo livre, que é a outra metade do que foi pedido.
    --
    -- Ele existe **junto** com as tags de propósito: as tags dizem o que você
    -- fez e a bio diz o que você quer dizer. Uma não substitui a outra, e o
    -- risco conhecido — a bio roubar a atenção do que foi conquistado — é
    -- resolvido na tela, não aqui: ela é uma linha, não um parágrafo.
    bio text CHECK (bio IS NULL OR length(bio) <= 140),

    -- A vitrine: obras que a pessoa escolheu mostrar.
    --
    -- `uuid[]` e não uma tabela de junção porque **a ordem é o conteúdo** —
    -- vitrine é curadoria, e a terceira caixa estar em terceiro lugar é a
    -- escolha. Uma tabela precisaria de uma coluna `posicao` pra dizer o que o
    -- array já diz, e sem chave estrangeira a obra apagada some da tela sozinha
    -- (o perfil resolve os ids na leitura).
    vitrine uuid[] NOT NULL DEFAULT '{}' CHECK (cardinality(vitrine) <= 6),

    atualizado_em timestamptz NOT NULL DEFAULT now()
);

COMMENT ON COLUMN perfil.titulo IS
    'Chave de conquista. Quem confere se foi desbloqueada é src/conquistas.rs.';
COMMENT ON COLUMN perfil.vitrine IS
    'Ids de work, na ordem escolhida. Sem FK: obra apagada some da vitrine na leitura.';

-- ------------------------------------------------------------- o job da saga

-- A busca das sagas é um `job` como os outros — 548 chamadas ao TMDB não correm
-- dentro de uma requisição, e o §34 já fixou o molde: estado no banco, progresso
-- visível, cancelamento no ponto seguro e retomada pelo `WHERE`.
--
-- O que falta é o `kind` ser aceito. Sem isto, `Job::start` devolve `None` e o
-- botão da tela responde *"o banco recusou abrir o job"* — que é a mensagem que
-- o `aquecer_producao` já sabia dar, e que ninguém entenderia.
ALTER TABLE job DROP CONSTRAINT IF EXISTS job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check CHECK (kind IN (
    'scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
    'live_import', 'trivia', 'producao', 'saga'
));
