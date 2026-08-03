//! R32 — conquistas, experiência e nível.
//!
//! ## O que esta fase desfaz
//!
//! O §40 entregou um "placar" com quatro números, numa aba escondida, com um
//! aviso impresso na própria tela mandando ignorar o número:
//!
//! > *"Contar não é medir. (…) se este número começar a escolher o que você
//! > assiste, ignore-o."*
//!
//! Aquilo foi construído contrariado, e o argumento contra a gamificação ficou
//! registrado no `DESIGN.md` como se fosse posição do projeto. **Não é** — é
//! posição de quem escreveu, contra quem decide. O pedido é explícito: *"algo
//! parecido com as conquistas da Steam"*, com XP, nível, camadas e comparação
//! entre amigos.
//!
//! O aviso sai. Um produto que entrega uma feature e imprime na tela um pedido
//! de desculpas por ela não entregou a feature.
//!
//! ## A lista mora aqui, e é decisão de quem decide
//!
//! > *"quem escreve a lista é quem programa"*
//!
//! Então não há tabela de definições. Uma daria uma tela de administração pra
//! criar conquista — que ninguém pediu — e faria a regra virar dado, quando ela
//! é código: *"terminou dez filmes de terror"* é um `SELECT`, não uma linha.
//!
//! O banco guarda **só o desbloqueio** (`conquista_do_usuario`): a chave e o
//! instante.
//!
//! ## Duas consultas, e não cento e vinte
//!
//! Avaliar cada regra com a sua própria consulta seria uma ida ao banco por
//! conquista, a cada leitura de perfil. Em vez disso o avaliador levanta os
//! **fatos** de uma pessoa — contagens, máximos, sequências — em duas consultas,
//! e as regras são funções em cima dessa estrutura.
//!
//! O efeito colateral é o que torna o resto simples: uma regra nova é uma linha
//! na lista, e só precisa de consulta nova se pedir um fato que ainda não existe.
//!
//! ## O XP é derivado, e é por isso que tudo é retroativo
//!
//! Não há tabela de pontos, ledger nem job de recálculo. O nível de alguém é uma
//! função do que essa pessoa fez, lida na hora — então **as conquistas são
//! retroativas de graça**: no dia em que isto liga, quem já terminou dois filmes
//! já terminou dois filmes, sem backfill nenhum.
//!
//! Medido antes de escrever, e o número tempera a expectativa: o acervo tem
//! **129 eventos de reprodução, de uma pessoa só, 18 obras e 2 terminadas**.
//! Retroativo hoje abre as primeiras fáceis e mais nada. O sistema está sendo
//! construído para o histórico que ele vai criar, não para o que existe.

use std::collections::HashMap;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Em que prateleira da lista a conquista está.
///
/// As camadas são do pedido, e cada uma tem um trabalho diferente:
///
/// | camada | pra quê |
/// |---|---|
/// | `Facil` | dopamina. Desbloqueia quase sozinha, e existe pra a lista não abrir vazia |
/// | `Media` | o corpo da lista: pedem hábito, não façanha |
/// | `Dificil` | pedem meses |
/// | `Impossivel` | não são pra ser desbloqueadas. Um acervo de 17 mil obras tem que ter um fundo do poço visível |
/// | `Nivel` | marcos de XP, e se desbloqueiam sozinhos ao subir |
/// | `Saga` | trilogias e coleções — dependem do `metadata/saga.rs` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Camada {
    Facil,
    Media,
    Dificil,
    Impossivel,
    Nivel,
    Saga,
}

impl Camada {
    /// Quanto vale uma conquista desta camada.
    ///
    /// Fixo por camada, e não por conquista: um número por linha seria cento e
    /// vinte decisões arbitrárias, e a primeira coisa que alguém faria é
    /// comparar duas e achar uma injusta. A camada **é** a dificuldade.
    pub const fn pontos(self) -> i32 {
        match self {
            Camada::Facil => 10,
            Camada::Media => 40,
            Camada::Dificil => 150,
            Camada::Impossivel => 1000,
            // Marco de nível não vale XP: valer daria XP por ter XP, e o nível
            // subiria sozinho até o fim da lista.
            Camada::Nivel => 0,
            Camada::Saga => 80,
        }
    }
}

/// Uma conquista, como a tela a mostra.
#[derive(Debug, Clone, Serialize)]
pub struct Conquista {
    pub chave: &'static str,
    pub nome: &'static str,
    /// O que precisa ser feito, em português de menu. **Sempre dito**, mesmo nas
    /// impossíveis: uma conquista secreta é uma conquista que ninguém persegue.
    pub descricao: &'static str,
    pub camada: Camada,
    pub pontos: i32,
    /// Se ela também serve de **título** no perfil.
    ///
    /// Nem toda conquista vira título — "terminou o primeiro filme" não é uma
    /// identidade. As que viram são as que dizem alguma coisa sobre quem você é.
    pub titulo: bool,
    /// E se ela libera uma **tag** pra vitrine do perfil.
    pub tag: Option<&'static str>,
}

/// Os fatos de uma pessoa, levantados de uma vez.
///
/// Tudo que qualquer regra da lista precisa saber. Crescer esta estrutura é o
/// preço de uma conquista que pede um fato novo — e é um preço que se paga uma
/// vez, não a cada regra que usa o mesmo fato.
#[derive(Debug, Default, Clone)]
pub struct Fatos {
    pub terminadas: i64,
    pub comecadas: i64,
    /// Minutos de filme terminado. Aproximação honesta: a duração da obra, não
    /// o tempo de tela — quem assiste em 1,5× assistiu o filme.
    pub minutos: i64,
    /// Terminadas por estante da locadora (o mesmo `ESTANTES` do §36).
    pub por_estante: HashMap<i32, i64>,
    /// Terminadas por década de lançamento.
    pub por_decada: HashMap<i32, i64>,
    /// Terminadas que são VHS (§35: ano ≤ 1996).
    pub vhs: i64,
    /// Obras terminadas mais de uma vez.
    pub reassistidas: i64,
    /// Terminadas entre meia-noite e cinco da manhã.
    pub madrugada: i64,
    /// O maior número de obras terminadas num mesmo dia.
    pub maior_maratona: i64,
    /// Dias seguidos com pelo menos uma obra terminada, hoje.
    pub sequencia: i64,
    /// O maior que essa sequência já foi.
    pub maior_sequencia: i64,
    pub emprestimos: i64,
    pub devolvidas_rebobinadas: i64,
    pub devolvidas_atrasadas: i64,
    /// Fitas suas que **alguém teve que rebobinar** (R30).
    pub zoadas: i64,
    /// Fitas dos outros que **você** rebobinou.
    pub rebobinou: i64,
    pub avaliacoes: i64,
    /// Avaliações com texto — resenha de verdade.
    pub resenhas: i64,
    pub amigos: i64,
    /// Sagas em que **todas** as obras do acervo foram terminadas.
    pub sagas_completas: i64,
    /// Séries em que todos os episódios do acervo foram terminados.
    pub series_completas: i64,
    /// Quantas obras o acervo tem, pra as impossíveis terem denominador.
    pub acervo: i64,
    /// O XP que os desafios cumpridos renderam (R35).
    ///
    /// Somado da **linha**, e não da definição: mudar o valor de um desafio
    /// amanhã não deve reescrever o XP de quem cumpriu ontem.
    pub xp_de_desafios: i64,
    /// Quantos desafios essa pessoa cumpriu.
    pub desafios: i64,
    /// De quantos **eventos do guia** essa pessoa participou (R34).
    ///
    /// É o único fato desta struct que não dá pra recalcular: a janela do evento
    /// fecha, e "terminou enquanto estava em cartaz" vira irrecuperável. Por
    /// isso ele é o único que vem de uma tabela em vez de uma contagem.
    pub eventos: i64,
}

/// A regra: uma função dos fatos.
type Regra = fn(&Fatos) -> bool;

/// A lista.
///
/// **Longa de propósito** — foi pedida "bem longa" —, e organizada por assunto e
/// não por camada: quem lê o código procura "as de terror", não "as médias".
///
/// A ordem desta lista é a ordem em que a tela mostra. Conquista nova entra no
/// fim do seu grupo; mexer na ordem não quebra nada, mas mexer numa **chave**
/// órfã um desbloqueio que já existe.
pub const LISTA: &[(Conquista, Regra)] = &[
    // ------------------------------------------------------- os primeiros passos
    (c("primeira", "A primeira fita", "Termine uma obra", Camada::Facil, false, None), |f| f.terminadas >= 1),
    (c("cinco", "Freguês", "Termine 5 obras", Camada::Facil, false, None), |f| f.terminadas >= 5),
    (c("vinte", "Sócio", "Termine 20 obras", Camada::Media, false, None), |f| f.terminadas >= 20),
    (c("cem", "Cinéfilo", "Termine 100 obras", Camada::Dificil, true, Some("cinéfilo")), |f| f.terminadas >= 100),
    (c("quinhentas", "Arquivista", "Termine 500 obras", Camada::Impossivel, true, Some("arquivista")), |f| f.terminadas >= 500),
    // Começar e não terminar também é um fato sobre alguém, e é o mais honesto
    // desta lista — o §8f já dizia que terminar é o sinal, então largar é o
    // outro. Ela não julga: só conta.
    (c("provador", "Provador", "Comece 50 obras", Camada::Media, false, None), |f| f.comecadas >= 50),
    (c("indeciso", "Indeciso", "Comece 200 obras", Camada::Dificil, true, Some("indeciso")), |f| f.comecadas >= 200),

    // ------------------------------------------------------------------ o tempo
    (c("dia", "Um dia inteiro", "Acumule 24 horas de obra terminada", Camada::Media, false, None), |f| f.minutos >= 1440),
    (c("semana", "Uma semana", "Acumule 168 horas", Camada::Dificil, false, None), |f| f.minutos >= 10_080),
    (c("mes", "Um mês na frente da tela", "Acumule 720 horas", Camada::Impossivel, true, Some("insone")), |f| f.minutos >= 43_200),
    (c("maratona3", "Sessão dupla e meia", "Termine 3 obras no mesmo dia", Camada::Facil, false, None), |f| f.maior_maratona >= 3),
    (c("maratona6", "Maratonista", "Termine 6 obras no mesmo dia", Camada::Media, true, Some("maratonista")), |f| f.maior_maratona >= 6),
    (c("maratona12", "Sem sair do sofá", "Termine 12 obras no mesmo dia", Camada::Dificil, true, None), |f| f.maior_maratona >= 12),
    (c("madrugada", "Sessão da madrugada", "Termine 10 obras entre meia-noite e 5h", Camada::Media, true, Some("madrugada")), |f| f.madrugada >= 10),
    (c("madrugada50", "O turno da noite", "Termine 50 obras de madrugada", Camada::Dificil, true, None), |f| f.madrugada >= 50),

    // -------------------------------------------------------------- a sequência
    (c("streak3", "Três noites", "Termine algo 3 dias seguidos", Camada::Facil, false, None), |f| f.maior_sequencia >= 3),
    (c("streak7", "Uma semana sem falhar", "Termine algo 7 dias seguidos", Camada::Media, false, None), |f| f.maior_sequencia >= 7),
    (c("streak30", "Trinta noites", "Termine algo 30 dias seguidos", Camada::Dificil, true, Some("disciplinado")), |f| f.maior_sequencia >= 30),
    (c("streak365", "Um ano de sessões", "Termine algo 365 dias seguidos", Camada::Impossivel, true, Some("possuído")), |f| f.maior_sequencia >= 365),

    // ---------------------------------------------------------------- as estantes
    //
    // Uma por estante da locadora, e o índice é o mesmo `ESTANTES` do §36 — a
    // mesma ordem que decide onde a caixa mora e qual menu de DVD ela abre.
    (c("terror10", "Não durmo mais", "Termine 10 de terror", Camada::Media, true, Some("terror")), |f| estante(f, 0) >= 10),
    (c("terror50", "Casa assombrada", "Termine 50 de terror", Camada::Dificil, true, None), |f| estante(f, 0) >= 50),
    (c("faroeste5", "Poeira", "Termine 5 faroestes", Camada::Media, false, Some("faroeste")), |f| estante(f, 1) >= 5),
    (c("guerra5", "Frente de batalha", "Termine 5 de guerra", Camada::Media, false, Some("guerra")), |f| estante(f, 2) >= 5),
    (c("doc10", "Documentado", "Termine 10 documentários", Camada::Media, true, Some("documentário")), |f| estante(f, 3) >= 10),
    (c("anim10", "Desenho animado", "Termine 10 animações", Camada::Media, true, Some("animação")), |f| estante(f, 4) >= 10),
    (c("anim50", "Estúdio inteiro", "Termine 50 animações", Camada::Dificil, true, None), |f| estante(f, 4) >= 50),
    (c("infantil10", "Sessão da tarde", "Termine 10 infantis", Camada::Media, false, Some("infantil")), |f| estante(f, 5) >= 10),
    (c("scifi10", "Contato", "Termine 10 de ficção científica", Camada::Media, true, Some("ficção")), |f| estante(f, 6) >= 10),
    (c("scifi50", "Outro planeta", "Termine 50 de ficção científica", Camada::Dificil, true, None), |f| estante(f, 6) >= 50),
    (c("acao10", "Explosão", "Termine 10 de ação", Camada::Media, false, Some("ação")), |f| estante(f, 7) >= 10),
    (c("crime10", "Cúmplice", "Termine 10 de crime e suspense", Camada::Media, true, Some("crime")), |f| estante(f, 8) >= 10),
    (c("comedia10", "Riso fácil", "Termine 10 comédias", Camada::Media, false, Some("comédia")), |f| estante(f, 9) >= 10),
    (c("romance10", "Coração mole", "Termine 10 romances", Camada::Media, false, Some("romance")), |f| estante(f, 10) >= 10),
    (c("drama10", "Coisa séria", "Termine 10 dramas", Camada::Media, false, Some("drama")), |f| estante(f, 11) >= 10),
    // A que exige a loja inteira. Doze estantes, dez de cada — é a conquista que
    // pede que a pessoa saia do próprio gosto, que é o terceiro pilar (§1).
    (c("todas_estantes", "A loja inteira", "Termine 10 de cada uma das 12 estantes", Camada::Impossivel, true, Some("onívoro")),
     |f| (0..12).all(|i| estante(f, i) >= 10)),

    // ----------------------------------------------------------------- as décadas
    (c("decada_40", "Preto e branco", "Termine algo dos anos 1940 ou antes", Camada::Media, false, None), |f| decada_ate(f, 1940) >= 1),
    (c("decada_60", "Clássico", "Termine 5 obras anteriores a 1970", Camada::Media, true, Some("clássico")), |f| decada_ate(f, 1960) >= 5),
    (c("decada_80", "Videocassete", "Termine 20 obras dos anos 80", Camada::Media, true, Some("oitentista")), |f| f.por_decada.get(&1980).copied().unwrap_or(0) >= 20),
    (c("decada_90", "Locadora de bairro", "Termine 20 obras dos anos 90", Camada::Media, false, Some("noventista")), |f| f.por_decada.get(&1990).copied().unwrap_or(0) >= 20),
    (c("sete_decadas", "Sete décadas", "Termine algo de 7 décadas diferentes", Camada::Dificil, true, Some("arqueólogo")), |f| f.por_decada.len() >= 7),

    // ------------------------------------------------------------------- a fita
    (c("vhs1", "Rebobine antes de devolver", "Termine uma fita", Camada::Facil, false, None), |f| f.vhs >= 1),
    (c("vhs20", "Fita gasta", "Termine 20 fitas", Camada::Media, true, Some("vhs")), |f| f.vhs >= 20),
    (c("gentil", "Gente boa", "Rebobine 10 fitas que outra pessoa deixou no meio", Camada::Media, true, Some("rebobinador")), |f| f.rebobinou >= 10),
    (c("gentil50", "O zelador", "Rebobine 50 fitas dos outros", Camada::Dificil, true, None), |f| f.rebobinou >= 50),
    // O outro lado, e ele não é castigo: é fato sobre pessoa real, que é o que a
    // R30 construiu. Quem devolve zoado carrega isso, e a tag é escolha dela.
    (c("relapso", "Devolveu zoado", "Deixe 10 fitas no meio pra outra pessoa rebobinar", Camada::Media, true, Some("relapso")), |f| f.zoadas >= 10),
    (c("relapso50", "Nunca rebobina", "Deixe 50 fitas no meio", Camada::Dificil, true, None), |f| f.zoadas >= 50),

    // --------------------------------------------------------------- a locadora
    (c("aluguel1", "Cliente novo", "Pegue uma caixa emprestada", Camada::Facil, false, None), |f| f.emprestimos >= 1),
    (c("aluguel25", "Carteirinha", "Pegue 25 caixas emprestadas", Camada::Media, false, None), |f| f.emprestimos >= 25),
    (c("aluguel100", "Freguês da casa", "Pegue 100 caixas emprestadas", Camada::Dificil, true, Some("freguês")), |f| f.emprestimos >= 100),
    (c("pontual", "Sempre no prazo", "Devolva 25 caixas rebobinadas", Camada::Media, true, Some("pontual")), |f| f.devolvidas_rebobinadas >= 25),
    (c("atrasado", "A multa", "Devolva 10 caixas atrasadas", Camada::Media, true, Some("atrasado")), |f| f.devolvidas_atrasadas >= 10),

    // ----------------------------------------------------------------- a opinião
    (c("nota1", "Primeira nota", "Avalie uma obra", Camada::Facil, false, None), |f| f.avaliacoes >= 1),
    (c("nota25", "Tem opinião", "Avalie 25 obras", Camada::Media, false, None), |f| f.avaliacoes >= 25),
    (c("nota200", "Crítico", "Avalie 200 obras", Camada::Dificil, true, Some("crítico")), |f| f.avaliacoes >= 200),
    (c("resenha1", "Escreveu", "Escreva uma resenha", Camada::Facil, false, None), |f| f.resenhas >= 1),
    (c("resenha25", "Colunista", "Escreva 25 resenhas", Camada::Media, true, Some("colunista")), |f| f.resenhas >= 25),
    (c("resenha100", "O jornal da casa", "Escreva 100 resenhas", Camada::Dificil, true, None), |f| f.resenhas >= 100),

    // ------------------------------------------------------------------ o social
    (c("amigo1", "Não está sozinho", "Faça um amigo", Camada::Facil, false, None), |f| f.amigos >= 1),
    (c("amigo5", "Turma", "Faça 5 amigos", Camada::Media, false, None), |f| f.amigos >= 5),

    // ------------------------------------------------------------- reassistir
    //
    // O §8f chama reassistir de "o sinal positivo mais forte que existe", e a
    // curadoria já o usa. Aqui ele vira reconhecimento em vez de só peso.
    (c("denovo", "De novo", "Termine a mesma obra duas vezes", Camada::Facil, false, None), |f| f.reassistidas >= 1),
    (c("denovo10", "Fita favorita", "Reassista 10 obras", Camada::Media, true, Some("saudosista")), |f| f.reassistidas >= 10),
    (c("denovo50", "Sabe os diálogos", "Reassista 50 obras", Camada::Dificil, true, None), |f| f.reassistidas >= 50),

    // ------------------------------------------------------------------- sagas
    //
    // Dependem de `metadata/saga.rs` ter rodado. Sem sagas no banco elas ficam
    // trancadas — o que é honesto: a lista mostra o que existe pra perseguir.
    (c("saga1", "A trilogia", "Termine todas as obras de uma saga", Camada::Saga, false, None), |f| f.sagas_completas >= 1),
    (c("saga3", "Colecionador", "Complete 3 sagas", Camada::Saga, true, Some("colecionador")), |f| f.sagas_completas >= 3),
    (c("saga10", "A estante inteira", "Complete 10 sagas", Camada::Dificil, true, None), |f| f.sagas_completas >= 10),
    (c("serie1", "Do piloto ao final", "Termine uma série inteira", Camada::Media, false, None), |f| f.series_completas >= 1),
    (c("serie5", "Maratonista de série", "Termine 5 séries inteiras", Camada::Dificil, true, Some("seriador")), |f| f.series_completas >= 5),

    // ------------------------------------------------------- os desafios
    //
    // Tarefas com prazo, sorteadas por pessoa (R35). Falhar não custa nada, e
    // por isso não há conquista de "não falhou" — ela seria a sequência que a
    // decisão recusou, entrando pela porta dos fundos.
    (c("desafio1", "Topou", "Cumpra um desafio", Camada::Facil, false, None), |f| f.desafios >= 1),
    (c("desafio10", "Dez tarefas", "Cumpra 10 desafios", Camada::Media, false, None), |f| f.desafios >= 10),
    (c("desafio50", "Sempre topa", "Cumpra 50 desafios", Camada::Dificil, true, Some("topador")), |f| f.desafios >= 50),
    (c("desafio200", "Nada fica pra depois", "Cumpra 200 desafios", Camada::Impossivel, true, None), |f| f.desafios >= 200),

    // ------------------------------------------------- os eventos do guia
    //
    // *"Os eventos temáticos do guia também concedem"* — e é o único grupo desta
    // lista que exige estar presente numa janela, e não acumular ao longo do
    // tempo. Quem chegou depois não pega.
    (c("evento1", "Esteve lá", "Participe de um evento da semana", Camada::Facil, false, None), |f| f.eventos >= 1),
    (c("evento5", "Figurinha carimbada", "Participe de 5 eventos", Camada::Media, true, Some("presente")), |f| f.eventos >= 5),
    (c("evento20", "Nunca falta", "Participe de 20 eventos", Camada::Dificil, true, Some("assíduo")), |f| f.eventos >= 20),
    (c("evento52", "Um ano de sessões marcadas", "Participe de 52 eventos", Camada::Impossivel, true, Some("cartaz")), |f| f.eventos >= 52),

    // ------------------------------------------------------------------ o fundo
    //
    // A impossível de verdade: o acervo inteiro. Ela existe pra ter fundo do
    // poço visível — 17.498 obras não vão ser assistidas por ninguém, e é
    // exatamente por isso que ela precisa estar na lista.
    (c("acervo", "Zerou o Odeon", "Termine tudo que existe neste servidor", Camada::Impossivel, true, Some("zerou")),
     |f| f.acervo > 0 && f.terminadas >= f.acervo),

    // ---------------------------------------------------------- marcos de nível
    //
    // Não valem XP (ver `Camada::pontos`) e não têm regra própria: quem as
    // desbloqueia é o nível, calculado do XP das outras. É o único lugar da
    // lista em que a regra olha um fato derivado — e por isso elas são as
    // últimas, avaliadas depois que o resto já somou.
    (c("nivel5", "Nível 5", "Chegue ao nível 5", Camada::Nivel, false, None), |_| false),
    (c("nivel10", "Nível 10", "Chegue ao nível 10", Camada::Nivel, true, None), |_| false),
    (c("nivel20", "Nível 20", "Chegue ao nível 20", Camada::Nivel, true, Some("veterano")), |_| false),
    (c("nivel40", "Nível 40", "Chegue ao nível 40", Camada::Nivel, true, Some("lenda")), |_| false),
];

/// Açúcar pra a lista caber numa linha por conquista.
const fn c(
    chave: &'static str,
    nome: &'static str,
    descricao: &'static str,
    camada: Camada,
    titulo: bool,
    tag: Option<&'static str>,
) -> Conquista {
    Conquista { chave, nome, descricao, camada, pontos: camada.pontos(), titulo, tag }
}

fn estante(f: &Fatos, i: i32) -> i64 {
    f.por_estante.get(&i).copied().unwrap_or(0)
}

fn decada_ate(f: &Fatos, ano: i32) -> i64 {
    f.por_decada.iter().filter(|(d, _)| **d <= ano).map(|(_, n)| *n).sum()
}

/// O nível de um XP.
///
/// Curva triangular: o nível `n` começa em `50·n·(n−1)` — 0, 100, 300, 600,
/// 1000, 1500… Cada nível custa 100 XP a mais que o anterior.
///
/// **Por que não linear.** Nível linear faz o número virar uma segunda contagem
/// de filmes, e aí ele não diz nada que "127 obras" já não dissesse. A curva é o
/// que faz o nível 20 significar outra coisa que o nível 10.
///
/// **E por que não exponencial.** Porque a lista tem fundo: com XP máximo perto
/// de 8.000, uma curva exponencial deixaria metade dos níveis inalcançáveis e a
/// outra metade grátis.
pub fn nivel_de(xp: i64) -> i32 {
    // n = floor((1 + sqrt(1 + xp/12.5)) / 2), invertendo 50·n·(n−1).
    let n = ((1.0 + (1.0 + (xp.max(0) as f64) / 12.5).sqrt()) / 2.0).floor() as i32;
    n.max(1)
}

/// Onde começa um nível.
pub fn xp_do_nivel(n: i32) -> i64 {
    let n = n.max(1) as i64;
    50 * n * (n - 1)
}

/// O XP de atividade, o que não vem de conquista.
///
/// Existe pra o nível **andar** entre uma conquista e outra. Só com pontos de
/// conquista, o XP ficaria parado por semanas e o número deixaria de significar
/// atividade — que é a única coisa que ele deve significar.
///
/// Os pesos são pequenos de propósito: a conquista é o marco, isto é o passo.
pub fn xp_de_atividade(f: &Fatos) -> i64 {
    f.terminadas * 10
        + f.comecadas * 2
        + f.avaliacoes * 5
        + f.resenhas * 10
        + f.emprestimos * 5
        + f.rebobinou * 5
        + f.series_completas * 25
        + f.sagas_completas * 25
        // O evento vale mais que um filme qualquer porque ele é coletivo e tem
        // hora: quem participou estava lá **naquela** semana (§2.4).
        + f.eventos * 20
        // E o desafio traz o próprio valor: cada linha guarda o XP com que
        // nasceu (R35).
        + f.xp_de_desafios
}

/// Tudo que o perfil precisa dizer sobre uma pessoa.
#[derive(Debug, Serialize)]
pub struct Progresso {
    pub xp: i64,
    pub nivel: i32,
    /// Onde o nível atual começou e onde o próximo começa — a tela desenha a
    /// barra com os dois, em vez de refazer a curva do lado dela.
    pub xp_do_nivel: i64,
    pub xp_do_proximo: i64,
    pub desbloqueadas: usize,
    pub total: usize,
}

/// Quem já desbloqueou o quê.
pub async fn desbloqueadas(pool: &PgPool, user_id: Uuid) -> HashMap<String, chrono::DateTime<chrono::Utc>> {
    sqlx::query_as::<_, (String, chrono::DateTime<chrono::Utc>)>(
        "SELECT chave, em FROM conquista_do_usuario WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Avalia a lista inteira e grava o que passou a valer.
///
/// **Idempotente e barato**: dois `SELECT` pra levantar os fatos, a lista
/// avaliada em memória, e um `INSERT … ON CONFLICT DO NOTHING` com o que faltava.
/// Chamar duas vezes seguidas não muda nada — a segunda não insere linha nenhuma.
///
/// Devolve as chaves que acabaram de abrir, pra quem chamou poder avisar.
pub async fn avaliar(pool: &PgPool, user_id: Uuid) -> Vec<&'static Conquista> {
    let f = match fatos(pool, user_id).await {
        Ok(f) => f,
        // Conquista não é caminho crítico: se o levantamento falhar, o resto da
        // requisição segue. Um perfil sem medalha nova é melhor que um 500.
        Err(e) => {
            tracing::warn!(erro = %e, "não deu pra levantar os fatos");
            return Vec::new();
        }
    };

    let ja = desbloqueadas(pool, user_id).await;

    // Primeira passada: tudo que não é marco de nível.
    let mut novas: Vec<&'static Conquista> = LISTA
        .iter()
        .filter(|(q, _)| q.camada != Camada::Nivel)
        .filter(|(q, regra)| !ja.contains_key(q.chave) && regra(&f))
        .map(|(q, _)| q)
        .collect();

    // O XP considera o que **já estava** aberto mais o que abriu agora — senão o
    // marco de nível chegaria sempre uma avaliação atrasado.
    let pontos: i32 = LISTA
        .iter()
        .filter(|(q, _)| ja.contains_key(q.chave) || novas.iter().any(|n| n.chave == q.chave))
        .map(|(q, _)| q.pontos)
        .sum();
    let nivel = nivel_de(pontos as i64 + xp_de_atividade(&f));

    for (q, _) in LISTA.iter().filter(|(q, _)| q.camada == Camada::Nivel) {
        let exigido: i32 = q.chave.trim_start_matches("nivel").parse().unwrap_or(i32::MAX);
        if nivel >= exigido && !ja.contains_key(q.chave) {
            novas.push(q);
        }
    }

    if !novas.is_empty() {
        let chaves: Vec<&str> = novas.iter().map(|q| q.chave).collect();
        let _ = sqlx::query(
            "INSERT INTO conquista_do_usuario (user_id, chave)
             SELECT $1, unnest($2::text[])
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(&chaves)
        .execute(pool)
        .await;
        tracing::info!(quantas = novas.len(), "conquistas desbloqueadas");
    }

    novas
}

/// O progresso de alguém, já com a lista avaliada.
pub async fn progresso(pool: &PgPool, user_id: Uuid) -> Progresso {
    let f = fatos(pool, user_id).await.unwrap_or_default();
    let ja = desbloqueadas(pool, user_id).await;
    let pontos: i64 = LISTA
        .iter()
        .filter(|(q, _)| ja.contains_key(q.chave))
        .map(|(q, _)| q.pontos as i64)
        .sum();
    let xp = pontos + xp_de_atividade(&f);
    let nivel = nivel_de(xp);
    Progresso {
        xp,
        nivel,
        xp_do_nivel: xp_do_nivel(nivel),
        xp_do_proximo: xp_do_nivel(nivel + 1),
        desbloqueadas: ja.len(),
        total: LISTA.len(),
    }
}

/// Levanta os fatos de uma pessoa.
///
/// Duas consultas: a primeira é sobre obras terminadas (e precisa do gênero, do
/// ano e da duração, então cruza `work`), a segunda é o resto — locadora, fita,
/// nota, amizade — que são contagens independentes e cabem num `SELECT` só de
/// subconsultas.
///
/// **"Terminada" é o §8f**, a mesma definição da curadoria, do guia, da locadora
/// e do mural: evento `finish` **ou** passar de 92%. Escrever outra aqui faria a
/// conquista discordar do mural sobre a palavra.
async fn fatos(pool: &PgPool, user_id: Uuid) -> Result<Fatos, sqlx::Error> {
    let mut f = Fatos::default();

    // --- as obras terminadas, uma linha por obra ---
    #[derive(sqlx::FromRow)]
    struct Linha {
        estante: Option<i32>,
        decada: Option<i32>,
        minutos: Option<f64>,
        vhs: bool,
        reassistida: bool,
        madrugada: bool,
        dia: chrono::NaiveDate,
    }

    let idx: Vec<i32> = crate::routes::locadora::ESTANTES
        .iter()
        .enumerate()
        .flat_map(|(i, (_, gs))| gs.iter().map(move |_| i as i32))
        .collect();
    let gen: Vec<String> = crate::routes::locadora::ESTANTES
        .iter()
        .flat_map(|(_, gs)| gs.iter().map(|g| g.to_string()))
        .collect();

    let linhas: Vec<Linha> = sqlx::query_as(
        r#"
        WITH terminadas AS (
            SELECT pe.work_id, max(pe.created_at) AS quando
            FROM play_event pe
            WHERE pe.user_id = $1
            GROUP BY pe.work_id
            HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
                OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
        )
        SELECT (SELECT min(e.idx) FROM work_tag wt
                  JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'genre'
                  JOIN LATERAL (SELECT * FROM unnest($2::int[], $3::text[]) AS x(idx, genero)) e
                    ON t.value = e.genero
                 WHERE wt.work_id = w.id)                       AS estante,
               (w.year / 10) * 10                               AS decada,
               COALESCE(w.runtime_seconds::float8 / 60.0, 0)    AS minutos,
               (w.year IS NOT NULL AND w.year <= $4)            AS vhs,
               COALESCE(ps.play_count, 0) > 1                   AS reassistida,
               EXTRACT(hour FROM t.quando AT TIME ZONE 'America/Sao_Paulo') < 5 AS madrugada,
               (t.quando AT TIME ZONE 'America/Sao_Paulo')::date AS dia
        FROM terminadas t
        JOIN work w ON w.id = t.work_id
        LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $1
        "#,
    )
    .bind(user_id)
    .bind(&idx)
    .bind(&gen)
    .bind(crate::routes::locadora::ULTIMO_ANO_VHS)
    .fetch_all(pool)
    .await?;

    f.terminadas = linhas.len() as i64;
    let mut dias: Vec<chrono::NaiveDate> = Vec::with_capacity(linhas.len());
    for l in &linhas {
        if let Some(e) = l.estante {
            *f.por_estante.entry(e).or_insert(0) += 1;
        }
        if let Some(d) = l.decada {
            *f.por_decada.entry(d).or_insert(0) += 1;
        }
        f.minutos += l.minutos.unwrap_or(0.0).round() as i64;
        f.vhs += l.vhs as i64;
        f.reassistidas += l.reassistida as i64;
        f.madrugada += l.madrugada as i64;
        dias.push(l.dia);
    }

    // A maratona e a sequência saem dos dias, e não do banco: são duas contas
    // sobre a mesma lista que já veio, e fazê-las em SQL custaria duas janelas
    // pra responder o que um `sort` responde.
    dias.sort_unstable();
    let (maior_dia, maior_seq, seq_hoje) = sequencias(&dias);
    f.maior_maratona = maior_dia;
    f.maior_sequencia = maior_seq;
    f.sequencia = seq_hoje;

    // --- o resto, em contagens independentes ---
    let (comecadas, emp, reb, atr, zoadas, rebobinou, aval, res, amigos, acervo, eventos, desafios, xp_desafios): (
        i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64,
    ) = sqlx::query_as(
        r#"
        SELECT
          (SELECT count(*) FROM playback_state WHERE user_id = $1 AND position_seconds > 60),
          (SELECT count(*) FROM emprestimo WHERE user_id = $1),
          (SELECT count(*) FROM emprestimo WHERE user_id = $1 AND devolvido_como = 'rebobinada'),
          (SELECT count(*) FROM emprestimo WHERE user_id = $1 AND devolvido_em > vence_em),
          (SELECT count(*) FROM rebobinada WHERE de = $1 AND por IS DISTINCT FROM $1),
          (SELECT count(*) FROM rebobinada WHERE por = $1 AND de IS DISTINCT FROM $1),
          (SELECT count(*) FROM avaliacao WHERE user_id = $1),
          (SELECT count(*) FROM avaliacao WHERE user_id = $1 AND texto IS NOT NULL),
          (SELECT count(*) FROM amizade WHERE aceito_em IS NOT NULL AND (a = $1 OR b = $1)),
          (SELECT count(*) FROM work WHERE match_state <> 'ignored'),
          (SELECT count(*) FROM evento_participacao WHERE user_id = $1),
          (SELECT count(*) FROM desafio WHERE user_id = $1 AND cumprido_em IS NOT NULL),
          COALESCE((SELECT sum(xp) FROM desafio WHERE user_id = $1 AND cumprido_em IS NOT NULL), 0)::bigint
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    f.comecadas = comecadas;
    f.emprestimos = emp;
    f.devolvidas_rebobinadas = reb;
    f.devolvidas_atrasadas = atr;
    f.zoadas = zoadas;
    f.rebobinou = rebobinou;
    f.avaliacoes = aval;
    f.resenhas = res;
    f.amigos = amigos;
    f.acervo = acervo;
    f.eventos = eventos;
    f.desafios = desafios;
    f.xp_de_desafios = xp_desafios;

    // --- as coleções completas ---
    //
    // "Completa" é **do acervo**, não do mundo: quem terminou os três filmes de
    // uma trilogia que este servidor tem completou a trilogia daqui. Exigir o
    // catálogo do TMDB faria a conquista depender de o dono ter comprado tudo.
    let (sagas, series): (i64, i64) = sqlx::query_as(
        r#"
        WITH terminadas AS (
            SELECT pe.work_id FROM play_event pe WHERE pe.user_id = $1
            GROUP BY pe.work_id
            HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
                OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
        ),
        colecoes AS (
            SELECT c.id, c.kind,
                   count(*) AS obras,
                   count(*) FILTER (WHERE t.work_id IS NOT NULL) AS feitas
            FROM collection c
            JOIN collection_item ci ON ci.collection_id = c.id
            LEFT JOIN terminadas t ON t.work_id = ci.work_id
            WHERE c.kind IN ('franchise', 'series')
            GROUP BY c.id, c.kind
        )
        SELECT
          count(*) FILTER (WHERE kind = 'franchise' AND obras > 1 AND obras = feitas),
          count(*) FILTER (WHERE kind = 'series'    AND obras > 1 AND obras = feitas)
        FROM colecoes
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    f.sagas_completas = sagas;
    f.series_completas = series;

    Ok(f)
}

/// A maior maratona, a maior sequência e a sequência viva.
///
/// Recebe os dias **ordenados**, com repetição — um dia aparece uma vez por obra
/// terminada nele.
///
/// "Viva" é a sequência que inclui hoje ou ontem. Ontem conta porque quem
/// assistiu ontem à noite e abre o perfil de manhã não perdeu a sequência —
/// perder por checar o placar cedo demais seria o número punindo o olhar.
fn sequencias(dias: &[chrono::NaiveDate]) -> (i64, i64, i64) {
    if dias.is_empty() {
        return (0, 0, 0);
    }
    let mut maior_dia = 1i64;
    let mut no_dia = 1i64;
    let mut unicos: Vec<chrono::NaiveDate> = Vec::new();

    for (i, d) in dias.iter().enumerate() {
        if i > 0 && *d == dias[i - 1] {
            no_dia += 1;
            maior_dia = maior_dia.max(no_dia);
        } else {
            no_dia = 1;
            unicos.push(*d);
        }
    }

    let mut maior_seq = 1i64;
    let mut atual = 1i64;
    for i in 1..unicos.len() {
        if unicos[i] - unicos[i - 1] == chrono::Duration::days(1) {
            atual += 1;
            maior_seq = maior_seq.max(atual);
        } else {
            atual = 1;
        }
    }

    let hoje = chrono::Utc::now().date_naive();
    let ultimo = *unicos.last().unwrap();
    let viva = if hoje - ultimo <= chrono::Duration::days(1) { atual } else { 0 };

    (maior_dia, maior_seq, viva)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Nenhuma chave repetida.** Duas conquistas com a mesma chave fariam a
    /// segunda ser inalcançável e a primeira desbloquear as duas na tela — e o
    /// `ON CONFLICT DO NOTHING` esconderia o defeito para sempre.
    #[test]
    fn as_chaves_sao_unicas() {
        let mut vistas = std::collections::HashSet::new();
        for (q, _) in LISTA {
            assert!(vistas.insert(q.chave), "chave repetida: {}", q.chave);
        }
    }

    /// A lista é pra ser **longa** — foi pedida assim. Este teste não julga o
    /// conteúdo; ele impede a lista de encolher sem alguém perceber.
    #[test]
    fn a_lista_e_longa_e_tem_todas_as_camadas() {
        assert!(LISTA.len() >= 60, "a lista encolheu: {}", LISTA.len());
        for camada in [
            Camada::Facil,
            Camada::Media,
            Camada::Dificil,
            Camada::Impossivel,
            Camada::Nivel,
            Camada::Saga,
        ] {
            assert!(
                LISTA.iter().any(|(q, _)| q.camada == camada),
                "camada sem nenhuma conquista: {camada:?}"
            );
        }
    }

    /// Marco de nível **não vale XP**. Se valesse, o nível daria XP por ter XP e
    /// subiria sozinho até o fim da lista — um laço com cara de recompensa.
    #[test]
    fn marco_de_nivel_nao_da_xp() {
        for (q, _) in LISTA.iter().filter(|(q, _)| q.camada == Camada::Nivel) {
            assert_eq!(q.pontos, 0, "{} dá XP e não devia", q.chave);
        }
    }

    /// A curva do nível e a sua inversa têm que concordar. Se divergirem, a
    /// barra do perfil enche antes ou depois de o nível virar — e o número
    /// passa a mentir sobre o próprio progresso.
    #[test]
    fn a_curva_do_nivel_fecha() {
        assert_eq!(nivel_de(0), 1);
        assert_eq!(xp_do_nivel(1), 0);
        for n in 1..60 {
            let inicio = xp_do_nivel(n);
            assert_eq!(nivel_de(inicio), n, "nível {n} começa em {inicio}");
            assert_eq!(nivel_de(inicio + 1), n);
            // Do nível 2 em diante: um XP a menos é o nível anterior. O 1 fica
            // de fora porque não há nível 0 — quem não fez nada é nível 1, e
            // XP negativo não existe.
            if n > 1 {
                assert_eq!(nivel_de(inicio - 1), n - 1, "a borda de baixo do {n} vazou");
            }
        }
        assert_eq!(nivel_de(-500), 1, "XP negativo tem que cair no nível 1");
        // E ela cresce: cada nível custa mais que o anterior.
        for n in 2..40 {
            let custo = xp_do_nivel(n + 1) - xp_do_nivel(n);
            let anterior = xp_do_nivel(n) - xp_do_nivel(n - 1);
            assert!(custo > anterior, "o nível {n} custou menos que o anterior");
        }
    }

    fn dia(s: &str) -> chrono::NaiveDate {
        s.parse().unwrap()
    }

    /// A maratona conta obras **no mesmo dia**; a sequência conta **dias
    /// seguidos**. São duas leituras da mesma lista, e trocá-las faria "termine
    /// 6 num dia" ser desbloqueada por seis dias seguidos.
    #[test]
    fn maratona_e_sequencia_sao_coisas_diferentes() {
        // Três num dia só: maratona 3, sequência 1.
        let d = vec![dia("2026-08-01"), dia("2026-08-01"), dia("2026-08-01")];
        let (m, s, _) = sequencias(&d);
        assert_eq!((m, s), (3, 1));

        // Três dias seguidos, um por dia: maratona 1, sequência 3.
        let d = vec![dia("2026-08-01"), dia("2026-08-02"), dia("2026-08-03")];
        let (m, s, _) = sequencias(&d);
        assert_eq!((m, s), (1, 3));

        // Um buraco no meio corta a sequência, e a maior sobrevive.
        let d = vec![
            dia("2026-08-01"), dia("2026-08-02"), dia("2026-08-03"),
            dia("2026-08-09"), dia("2026-08-10"),
        ];
        let (_, s, _) = sequencias(&d);
        assert_eq!(s, 3);

        assert_eq!(sequencias(&[]), (0, 0, 0));
    }

    /// A sequência **viva** morre quando o último dia é velho — mas ontem ainda
    /// conta. Sem a folga de um dia, abrir o perfil de manhã zeraria a
    /// sequência de quem assistiu ontem à noite: o número puniria o olhar.
    #[test]
    fn a_sequencia_viva_aceita_ontem_e_recusa_anteontem() {
        let hoje = chrono::Utc::now().date_naive();
        let ontem = hoje - chrono::Duration::days(1);
        let anteontem = hoje - chrono::Duration::days(2);

        assert_eq!(sequencias(&[anteontem, ontem, hoje]).2, 3);
        assert_eq!(sequencias(&[anteontem, ontem]).2, 2);
        // Terminou anteontem e parou: a sequência acabou.
        assert_eq!(sequencias(&[anteontem - chrono::Duration::days(1), anteontem]).2, 0);
    }

    /// Título e tag só saem de conquista que existe. Um título órfão no perfil
    /// seria a única mentira que ele poderia contar.
    #[test]
    fn os_titulos_e_tags_vem_da_lista() {
        let titulos = LISTA.iter().filter(|(q, _)| q.titulo).count();
        let tags = LISTA.iter().filter(|(q, _)| q.tag.is_some()).count();
        assert!(titulos >= 15, "poucos títulos pra escolher: {titulos}");
        assert!(tags >= 15, "poucas tags pra escolher: {tags}");
        // Nenhuma tag repetida — duas conquistas liberando a mesma etiqueta
        // fariam a vitrine mostrar a mesma palavra por dois motivos.
        let mut vistas = std::collections::HashSet::new();
        for (q, _) in LISTA {
            if let Some(t) = q.tag {
                assert!(vistas.insert(t), "tag repetida: {t}");
            }
        }
    }
}
