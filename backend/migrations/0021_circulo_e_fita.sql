-- R19 — o círculo e a fita.
--
-- A locadora da R8 (§20) é uma **vitrine**: 600 caixas lindas, estado nenhum.
-- Nada do que se faz lá deixa marca, e voltar amanhã encontra a mesma loja.
-- Esta migração é o que falta pra ela virar lugar: alguém está com a fita, e
-- ela volta em algum estado.
--
-- ## Por que o círculo, e não "a casa"
--
-- "A casa" nunca foi conceito do schema — era `SELECT * FROM app_user`. A
-- escassez precisava de um escopo pra deixar de ser mentira: "este DVD está
-- alugado" é falso quando o arquivo está sempre lá, e o §18 proíbe dizer coisa
-- falsa com cara de metadado. Dentro de um círculo deixa de ser falso — a fita
-- **está** com alguém, e é essa pessoa que está te barrando, não o software.
--
-- Adotar o círculo agora custa uma coluna e evita uma migração dolorosa:
-- empréstimo, rotação (R20), feed (R25) e retrospectiva (R24) são todos por
-- círculo. É a mesma jogada do `programme.work_id` do §17.
--
-- ## A tabela que este arquivo NÃO cria: `exemplar`
--
-- O plano (`IDEIAS.md` §2.2) previa `exemplar` — uma cópia de uma caixa dentro
-- de um círculo. Medido antes de escrever: o acervo tem **746 caixas com
-- pôster** (114 séries + 632 avulsas). Uma linha por caixa por círculo seriam
-- 746 linhas dizendo todas a mesma coisa — `copias = 1` — porque a decisão foi
-- **uma cópia por caixa**, que é a versão mais dramática e a que faz a escassez
-- existir.
--
-- Uma tabela cujas linhas não carregam informação nenhuma é enfeite de schema.
-- A escassez de uma cópia é um **índice único parcial** sobre o empréstimo em
-- aberto, e aí quem recusa o segundo aluguel é o banco, não uma checagem que
-- alguém pode esquecer de escrever no segundo caminho de código. É o argumento
-- do §5 pra `CHECK` em vez de validação na aplicação.
--
-- O dia em que uma caixa precisar de duas cópias, `exemplar` nasce — e nasce
-- carregando informação de verdade. Enquanto for sempre 1, ele é uma constante
-- com custo de JOIN.
--
-- ## A condição da fita também não vira coluna
--
-- Pelo mesmo motivo, e este é o melhor achado da rodada: **o estado da fita já
-- está no banco.** `playback_state` guarda, por usuário e por obra, onde a
-- pessoa parou. Quem assistiu até o minuto 47 e devolveu deixou a fita no
-- minuto 47 — isso é literalmente verdade, não simulação. Uma condição sorteada
-- seria enfeite; uma condição que é o progresso real de outra pessoa é
-- informação vestida de objeto.
--
-- O que a tabela guarda é só o que **não** dá pra derivar depois: como a fita
-- estava **no instante da devolução**. Isso precisa ser congelado porque o
-- `playback_state` continua andando — quem devolveu pode reassistir amanhã, e
-- aí a história de "voltou no meio" já teria sido reescrita.

-- ---------------------------------------------------------------- o círculo

CREATE TABLE circulo (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    nome       text NOT NULL,
    criado_em  timestamptz NOT NULL DEFAULT now(),

    -- Prazo em dias. Mora no círculo e não numa constante do código porque a
    -- primeira coisa que dois grupos diferentes vão querer é prazo diferente —
    -- e porque um número de regra de negócio escondido em `const` é um número
    -- que ninguém encontra.
    prazo_dias         int NOT NULL DEFAULT 7 CHECK (prazo_dias BETWEEN 1 AND 90),

    -- Quantas caixas cada membro segura ao mesmo tempo. O limite é a feature,
    -- não o obstáculo: prateleira infinita não faz ninguém escolher nada, e
    -- curadoria por restrição é o terceiro pilar (§1, e o "tenho 40 minutos"
    -- do §8f é o mesmo raciocínio).
    limite_por_membro  int NOT NULL DEFAULT 3 CHECK (limite_por_membro BETWEEN 1 AND 50)
);

CREATE TABLE circulo_membro (
    circulo_id uuid NOT NULL REFERENCES circulo(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    desde      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (circulo_id, user_id)
);

CREATE INDEX circulo_membro_user_idx ON circulo_membro (user_id);

-- ------------------------------------------------------------- o empréstimo

CREATE TABLE emprestimo (
    id          bigserial PRIMARY KEY,
    circulo_id  uuid NOT NULL REFERENCES circulo(id) ON DELETE CASCADE,
    user_id     uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- A caixa. A locadora agrupa como `/api/library` agrupa: uma série é uma
    -- caixa e não vinte e uma fitas (§20), então o alvo é uma obra avulsa **ou**
    -- a coleção da série.
    --
    -- Duas colunas anuláveis com um CHECK, e não um `caixa_id text` genérico,
    -- porque as duas mantêm chave estrangeira de verdade: apagar uma série
    -- limpa os empréstimos dela. Um id polimórfico em texto perderia isso e
    -- deixaria empréstimo órfão apontando pra nada.
    work_id       uuid REFERENCES work(id)       ON DELETE CASCADE,
    collection_id uuid REFERENCES collection(id) ON DELETE CASCADE,
    CONSTRAINT emprestimo_uma_caixa CHECK ((work_id IS NULL) <> (collection_id IS NULL)),

    pego_em   timestamptz NOT NULL DEFAULT now(),
    -- Todo empréstimo nasce com data de devolução. O prazo não é enfeite
    -- temático: ele é a **válvula que impede o impasse**. Sem ele, alguém que
    -- esquece de devolver tranca a outra pessoa fora do arquivo pra sempre, e
    -- num círculo com gente que você não vê todo dia isso não se resolve
    -- gritando pelo corredor. A locadora já tinha resolvido isso — chamava
    -- multa.
    vence_em  timestamptz NOT NULL,

    devolvido_em timestamptz,

    -- Como a fita voltou, congelado no instante da devolução (ver o cabeçalho).
    -- Derivado do `playback_state` de quem estava com ela, com a regra do §8f
    -- pra "terminada" — a mesma que a curadoria e o guia usam.
    devolvido_como text CHECK (devolvido_como IN ('rebobinada', 'no-meio', 'terminada')),

    -- Quem devolveu: o membro, ou o prazo. "Devolveu atrasado" é fato real
    -- sobre pessoa real, e é combustível legítimo pra retrospectiva da R24 —
    -- sem inventar métrica nenhuma.
    devolvido_por text CHECK (devolvido_por IN ('membro', 'prazo')),

    -- Quando alguém pediu de volta. Um bloqueio sem saída é uma parede; este
    -- tem porta. É registro e aviso, e **não encurta o prazo de ninguém** —
    -- dar a um membro poder sobre o prazo do outro transformaria a locadora
    -- em disputa.
    pedido_em      timestamptz,
    pedido_por     uuid REFERENCES app_user(id) ON DELETE SET NULL,

    -- Devolvido implica os dois campos de devolução preenchidos, e em aberto
    -- implica os dois vazios. Sem isto, "devolvida sem saber como" é um estado
    -- alcançável, e ele contaminaria a retrospectiva com linha pela metade.
    CONSTRAINT emprestimo_devolucao_completa CHECK (
        (devolvido_em IS NULL     AND devolvido_como IS NULL AND devolvido_por IS NULL)
     OR (devolvido_em IS NOT NULL AND devolvido_como IS NOT NULL AND devolvido_por IS NOT NULL)
    )
);

-- **A escassez, imposta pelo banco.** Uma cópia por caixa por círculo: só pode
-- existir um empréstimo em aberto da mesma caixa no mesmo círculo. São dois
-- índices porque são duas colunas de alvo, e o parcial (`WHERE devolvido_em IS
-- NULL`) é o que permite a mesma caixa ser alugada de novo depois de voltar.
--
-- Isto é a regra de negócio inteira em duas linhas de DDL. A alternativa —
-- conferir no código antes de inserir — tem corrida entre a conferência e o
-- INSERT, e este é o primeiro código do projeto onde duas pessoas disputam a
-- mesma linha de propósito.
CREATE UNIQUE INDEX emprestimo_uma_copia_work_idx
    ON emprestimo (circulo_id, work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL;

CREATE UNIQUE INDEX emprestimo_uma_copia_colecao_idx
    ON emprestimo (circulo_id, collection_id)
    WHERE devolvido_em IS NULL AND collection_id IS NOT NULL;

-- "O que está fora agora, neste círculo" — a pergunta que a locadora faz uma
-- vez por carregamento de tela.
CREATE INDEX emprestimo_em_aberto_idx
    ON emprestimo (circulo_id) WHERE devolvido_em IS NULL;

-- "Quantas eu tenho na mão" (o limite) e "meu histórico" (a R24).
CREATE INDEX emprestimo_user_idx ON emprestimo (user_id, pego_em DESC);

-- "O que venceu" — a varredura da devolução automática, que roda a cada leitura
-- da prateleira e portanto precisa ser barata.
CREATE INDEX emprestimo_vencendo_idx
    ON emprestimo (vence_em) WHERE devolvido_em IS NULL;

-- ------------------------------------------------------------------- a casa

-- A casa vira o primeiro círculo, com quem já existe dentro.
--
-- Não é seed de exemplo: são os 2 usuários reais deste servidor. O círculo
-- precisa existir antes da primeira tela porque a locadora sem círculo não tem
-- o que mostrar — e criar "o seu primeiro círculo" numa tela de onboarding
-- seria cerimônia pra dois usuários que já se conhecem.
INSERT INTO circulo (nome) VALUES ('A casa');

INSERT INTO circulo_membro (circulo_id, user_id)
SELECT c.id, u.id FROM circulo c CROSS JOIN app_user u WHERE c.nome = 'A casa';
