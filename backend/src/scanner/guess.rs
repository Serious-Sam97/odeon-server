//! Nome de arquivo (e de pasta) → estrutura.
//!
//! O diferencial nº1 sobre o Jellyfin. Três coisas que ele faz mal e aqui são
//! tratadas de propósito:
//!
//! 1. **Contexto de diretório.** `Severance/Season 2/S02E07.mkv` não tem título
//!    nenhum no arquivo — o título está duas pastas acima.
//! 2. **Anime.** `[SubsPlease] Frieren - 12 [1080p][A1B2].mkv` não tem temporada,
//!    tem episódio absoluto, e o `[SubsPlease]` não é parte do nome.
//! 3. **Nunca inventar.** Se não dá pra saber, o campo fica `None` e a obra cai
//!    na fila de revisão. Chutar em silêncio é o pecado do Jellyfin.

use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;

/// Ano plausível pra uma obra audiovisual. Serve pra não confundir o "2049" de
/// "Blade Runner 2049" com o ano de lançamento.
const MIN_YEAR: i32 = 1880;
const MAX_YEAR: i32 = 2035;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guess {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// Anime numera direto de 1 a N, ignorando temporada.
    pub absolute_episode: Option<i32>,
    pub release_group: Option<String>,
    pub looks_like_anime: bool,
}

impl Guess {
    /// O `kind` da obra: numeração manda, **menos onde a biblioteca já disse
    /// o que guarda** (R75).
    ///
    /// A regra original era "tem número de episódio → é episódio", e ela vale
    /// numa biblioteca de série. Numa de canal, não: um vídeo chamado
    /// `001 - Payday 2` continua sendo vídeo de canal, e virava `episode` só
    /// por ter número na frente.
    ///
    /// Medido em 20/08/2026: **329 obras** da biblioteca do YouTube estavam
    /// como episódio — os 331 dos Irmãos Piologo entre elas. Apareciam na
    /// prateleira `série` e ficavam **fora do canal**, que agrupa
    /// `kind = 'video'`.
    ///
    /// O número **não** se perde: ele continua em `season`/`episode` e é o que
    /// ordena o vídeo dentro da playlist.
    pub fn kind(&self, library_default: &str) -> String {
        if library_default == "video" {
            return library_default.to_string();
        }
        if self.episode.is_some() || self.absolute_episode.is_some() {
            "episode".to_string()
        } else {
            library_default.to_string()
        }
    }

    /// Qualquer numeração de episódio, pro matcher não precisar saber qual é.
    pub fn any_episode(&self) -> Option<i32> {
        self.episode.or(self.absolute_episode)
    }
}

static LEADING_GROUP: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[([^\]]{1,40})\]\s*").unwrap());

/// Endereço de site carimbado no nome pelo tracker: `WWW.BLUDV.COM`,
/// `hidratorrent.com`, `ondeeubaixo.com`.
///
/// Removido ANTES da normalização, e isso é o ponto: a normalização troca ponto
/// por espaço quando o nome não tem espaços, e aí `BLUDV.COM` vira `BLUDV COM`
/// — o domínio deixa de ser reconhecível. Aplicado cedo, some inteiro.
///
/// Sem isso o título buscado no provider vira
/// `Pica-Pau.WEB.DUB-WWW.BLUDV.COM`, que não casa com nada: 489 arquivos do
/// acervo caíam aqui.
/// O separador que vem depois do domínio entra na remoção, e isso importa:
/// `ondeeubaixo.com - Os.Jetsons.S01E12` sem ele viraria ` - Os.Jetsons.S01E12`,
/// que TEM espaço — e aí a heurística de scene release não converte os pontos
/// (ela existe pra proteger "Mr. Robot"). O título ficaria `Os.Jetsons`.
/// Levando o " - " junto, sobra `Os.Jetsons.S01E12`, sem espaço nenhum, e os
/// pontos viram espaços como devem.
static SITE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(?:www\.)?[a-z0-9][a-z0-9-]{1,30}\.(?:com|tv|net|org|me|to|cc|info|biz)\b\s*[-–—]?\s*",
    )
    .unwrap()
});

/// Token de release COLADO por ponto/underscore: `.WEB.`, `.DUB-`.
///
/// Ancorado no separador de propósito. `web` e `dub` soltos não entram no JUNK
/// global porque cortariam títulos legítimos — "Charlotte's Web" viraria
/// "Charlotte's". Colados por ponto eles nunca são título: é nomenclatura de
/// release que sobreviveu porque o nome misturava pontos e espaços, e por isso
/// a normalização não os converteu.
/// Sem lookahead: o crate `regex` não tem, de propósito — é o que garante o
/// tempo linear. O `\b` no fim resolve igual aqui.
static DOTTED_RELEASE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)[._-](?:web|dub|dl|leg)\b").unwrap());

/// `S01E02`, e também `T01E04` — a notação luso-brasileira de Temporada, que é
/// maioria em material dublado. O separador entre a letra E e o número é opcional
/// porque `S07 E 20` aparece solto no acervo.
///
/// O dígito TEM que vir colado ao `S`/`T`: sem isso "Part 1 e 2" viraria
/// temporada.
///
/// ## O `R` opcional, e de onde ele veio (R54)
///
/// `Arrested Development (2003) - S04RE01 - Re Cap'n Bluth.mkv`. O `RE` é a
/// **Remix** — a temporada 4 que a Netflix recortou e renumerou —, e o `R` no
/// meio quebrava a expressão inteira: 22 arquivos ficavam sem temporada e sem
/// episódio, e sem episódio o scanner não cria `collection(série)`. A biblioteca
/// mostrava 22 cartões idênticos em vez de uma série.
///
/// **Um `r?`, e não `[a-z]{0,2}`.** A versão genérica parecia melhor e criava um
/// falso positivo real: `S02 The 100` casaria como temporada 2, episódio 100 —
/// e "The 100" é uma série que existe. Uma letra conhecida, entre o número da
/// temporada e o `E`, não tem essa superfície.
static SEASON_EP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b[st](\d{1,2})[\s._-]*r?[\s._-]*e[\s._-]*(\d{1,3})\b").unwrap());

static SEASON_EP_X: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(\d{1,2})x(\d{2,3})\b").unwrap());

/// "3ª Temporada - Episódio 03", "Temporada 2 Ep 5" — por extenso, sem sigla.
static SEASON_EP_WORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        (?: (\d{1,2})\s*[ªºao]?\s*temporada | temporada\s*(\d{1,2}) )
        .{0,24}?
        (?: epis[oó]dio | \bep\b )\s*[._-]?\s*(\d{1,3})",
    )
    .unwrap()
});

/// "EP13", "Ep 22", "Episódio 12" — a sigla e a palavra inteira.
///
/// A palavra por extenso sem temporada junto é comum em material dublado
/// (`Episódio 12 - Ruína e Redenção de Apu.mkv`): 225 arquivos no acervo real
/// caíam fora só por isso, porque o `\bep` seguido de dígito não casa com
/// "Episódio" — depois do `ep` vem `i`.
///
/// Só em biblioteca de episódios: num acervo de filmes "EP" costuma ser disco
/// de música, não numeração.
static EP_PREFIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bep(?:is[oó]dio)?[\s._-]*(\d{1,3})\b").unwrap());

/// "Título - 12" / "Título - 12v2" — numeração absoluta de anime.
static ABSOLUTE_EP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s-\s*(\d{1,4})(?:v\d+)?\s*(?:\[|\(|$)").unwrap());

/// "031 - Dia de Ventania" — índice na FRENTE, e o que sobra é o nome do
/// episódio, não o da série. Por isso quem casa aqui abre mão do título: a
/// série tem que vir da pasta.
static LEADING_INDEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d{1,4})\s*[-–—.]\s+\S").unwrap());
static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:\b|\()(\d{4})(?:\b|\))").unwrap());
static BRACKETED: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\[\(\{][^\]\)\}]*[\]\)\}]").unwrap());
/// Pasta que só diz "temporada N" — não carrega o nome da obra.
///
/// Duas ordens, porque as duas aparecem no acervo real: "Season 2"/"Temporada 2"
/// e a forma luso-brasileira com o número na frente — "2 Temporada",
/// "2ª Temporada", "3a.Temporada". Aceita ainda o sufixo solto que vem de
/// release ("Season 1 (BluRay)", "2ª Temporada Completa").
///
/// **Ancorado no início de propósito.** "Breaking Bad (2008) Season 1-5" e
/// "Chaves - 7ª Temporada Completa (WEB-DL)" mencionam temporada mas COMEÇAM
/// com o nome da série — casá-las aqui jogaria fora justamente a informação que
/// se quer. Ficam de fora, com o comportamento de antes.
static SEASON_DIR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        ^(?:
            (?:season|temp(?:orada)?|saison|staffel|s)\s*[._-]?\s*(\d{1,2})
          |
            (\d{1,2})\s*[ªºao°]?\s*[._-]?\s*(?:temp(?:orada)?|season|saison|staffel)
        )
        (?:\s+completas?)?
        (?:\s*[\[\(][^\]\)]*[\]\)])?
        \s*$
        |^(?:specials?|especiais?)\s*$",
    )
    .unwrap()
});


/// `08-21 720p (Aguentando)` — temporada e episódio colados por um traço, no
/// começo do nome. **64 arquivos de `Dr House Dublado`** neste acervo.
///
/// O `INDICE_NA_FRENTE` não pegava porque exige espaço depois do traço, e mesmo
/// se pegasse leria `08` como índice absoluto — perdendo a temporada.
///
/// Dois dígitos de cada lado, no começo, e o que vem depois não pode ser dígito:
/// sem isso `1080-720p` viraria temporada 10.
static TEMP_EP_TRACO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d{1,2})-(\d{1,2})(?:\D|$)").unwrap());

/// `Pica-Pau.WEB.DUB-… (98).mkv` — o número absoluto entre parênteses, no fim.
/// **196 arquivos** neste acervo, e todos ficavam sem episódio nenhum.
///
/// **Ano é recusado na leitura, não aqui.** `Filme (2019).mkv` casa com esta
/// expressão, e é por isso que quem a usa confere a faixa `MIN_YEAR..=MAX_YEAR`
/// antes de acreditar. Separar as duas coisas na própria regex exigiria repetir
/// a definição de "ano plausível" num segundo lugar.
static EP_ENTRE_PARENTESES: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\((\d{1,4})\)\s*$").unwrap());

/// `1°Temp/12.avi` — o nome do arquivo é **só** o número. 14 arquivos de
/// `Kenan e Kel`, com a temporada na pasta.
///
/// Três dígitos no máximo: com quatro, `2012.avi` viraria episódio 2012 em vez
/// do filme que ele é.
static SO_O_NUMERO: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d{1,3})$").unwrap());

/// Pasta de especiais → temporada 0, que é a convenção do TMDB.
static SPECIALS_DIR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(specials?|especiais?)\s*$").unwrap());

/// Pasta de material extra. Não é temporada, mas também não é nome de obra —
/// é um contêiner, igual a "Season 2". Sem pulá-la, um episódio dentro de
/// `The Office/Featurettes/` passa a se chamar "Featurettes".
///
/// Não define temporada: "Deleted Scenes" não é a temporada 0 do TMDB, é
/// material que em geral não existe no provider nenhum.
static EXTRAS_DIR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        ^(?: featurettes? | extras? | bonus | b[oô]nus
           | deleted[\s._-]*scenes? | cenas[\s._-]*deletadas
           | promos? | trailers? | webisodes? | shorts? | interviews?
           | making[\s._-]*of | bloopers? | behind[\s._-]*the[\s._-]*scenes
        )\s*$",
    )
    .unwrap()
});

/// Tokens que sinalizam "daqui pra frente é metadata de release, não título".
static JUNK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        \b(
            2160p|1080p|720p|480p|uhd|hdr10\+?|hdr|
            x264|x265|h\.?264|h\.?265|hevc|avc|xvid|divx|10bit|8bit|
            bluray|blu-ray|bdrip|brrip|webrip|web-?dl|hdtv|dvdrip|remux|
            proper|repack|extended|unrated|imax|
            aac|ac3|eac3|truehd|atmos|dts(-hd)?|ddp?5[\s._-]?1|
            dual|dublado|legendado|nacional|multi|subbed
        )\b",
    )
    .unwrap()
});

static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

/// Parse só do nome do arquivo, sem contexto de diretório.
///
/// O scanner usa `guess_from_path`, que tem o contexto do diretório. Esta
/// versão serve pra rodar o parser sobre um nome ISOLADO — o nome de uma pasta,
/// em `routes::scopes`, onde justamente se quer o que a pasta diz sozinha.
pub fn guess_from_filename(filename: &str) -> Guess {
    guess_with_hint(filename, false, false)
}

/// Versão consciente do caminho: usa as pastas acima quando o arquivo sozinho
/// não diz nada, e detecta anime pela árvore de diretórios.
/// `serial` = a biblioteca guarda episódios (`library.default_kind`). É o que
/// libera as regras que só fazem sentido em série — índice na frente e episódio
/// absoluto sem sinal de anime. Num acervo de filmes elas causariam estrago.
pub fn guess_from_path(path: &Path, root: &Path, serial: bool) -> Guess {
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let relative = path.strip_prefix(root).unwrap_or(path);
    let anime_hint = relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_lowercase();
        name.contains("anime") || name.contains("animes")
    });

    let mut guess = guess_with_hint(&filename, anime_hint, serial);

    if !is_informative(&guess.title, serial) {
        // Sobe a árvore pulando pastas de temporada, até achar um nome que
        // signifique alguma coisa. Dois níveis cobrem Série/Temporada/arquivo.
        let mut current = path.parent();
        for _ in 0..2 {
            let Some(directory) = current else { break };
            if directory == root {
                break;
            }
            let name = directory
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();

            let trimmed = name.trim();
            if !SEASON_DIR.is_match(trimmed) && !EXTRAS_DIR.is_match(trimmed) {
                let from_directory = guess_with_hint(&name, anime_hint, serial);
                if is_informative(&from_directory.title, serial) {
                    guess.title = from_directory.title;
                    guess.year = guess.year.or(from_directory.year);
                    break;
                }
            }
            current = directory.parent();
        }
    }

    // Temporada vinda da PASTA, quando o arquivo não a diz.
    //
    // Sem isto o `guess_from_path` só resolvia o título, e `Show/Season 2/01.mkv`
    // continuava sem temporada — o que esvaziaria a gravação de
    // `work.season_number`. A pasta é justamente onde essa informação está no
    // layout mais comum.
    if guess.season.is_none() {
        if let Some(directory) = path.parent() {
            let name = directory
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = name.trim();

            if SPECIALS_DIR.is_match(name) {
                // Temporada 0 é a convenção do TMDB para especiais.
                guess.season = Some(0);
            } else if let Some(caps) = SEASON_DIR.captures(name) {
                // Grupo 1 = "Season 2"; grupo 2 = "2ª Temporada". Só um casa.
                guess.season = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .and_then(|m| m.as_str().parse().ok());
            }
        }
    }

    guess
}

/// Desfaz as trocas que o sistema de arquivos obrigou.
///
/// Nenhum sistema de arquivos aceita `/ \ : * ? " < >` num nome, e todo
/// downloader resolve isso do mesmo jeito: troca por um sósia Unicode que
/// *parece* o caractere e não é. O nome sobrevive no disco, e o título chega
/// aqui escrito errado — `Happy Tree Friends： Still Alive`,
/// `Snow What？ That's What`, `AC⧸DC`.
///
/// Medido neste acervo: **1.542 obras** com pelo menos um deles, quase 9% da
/// biblioteca. O `：` sozinho aparece em 1.246 — é o dois-pontos de subtítulo,
/// que quase toda série tem.
///
/// Desfazer é seguro aqui porque a entrada é, por definição, um nome de
/// arquivo ou de pasta: se o sósia está no nome, foi o downloader que o pôs.
pub fn restaura_o_que_o_disco_proibiu(nome: &str) -> String {
    if !nome.contains(|c| matches!(c, '⧸' | '／' | '⧵' | '＼' | '：' | '＊' | '？' | '＂' | '＜' | '＞' | '｜'))
    {
        return nome.to_string();
    }
    nome.chars()
        .map(|c| match c {
            '⧸' | '／' => '/',
            '⧵' | '＼' => '\\',
            '：' => ':',
            '＊' => '*',
            '？' => '?',
            '＂' => '"',
            '＜' => '<',
            '＞' => '>',
            '｜' => '|',
            outro => outro,
        })
        .collect()
}

fn guess_with_hint(filename: &str, anime_hint: bool, serial: bool) -> Guess {
    let filename = restaura_o_que_o_disco_proibiu(filename);
    let filename = filename.as_str();
    let stem = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename)
        .to_string();

    // Domínio e token de release fora antes de qualquer outra coisa — ver os
    // comentários do `SITE` e do `DOTTED_RELEASE`.
    //
    // Substituídos por VAZIO, não por espaço: introduzir um espaço aqui quebra
    // a heurística logo abaixo ("se não há espaço nenhum, os pontos SÃO os
    // espaços"). `A.Grande.Família.S03E27.WEB-DL` ganharia um espaço no meio e
    // os pontos deixariam de ser convertidos — o título viraria
    // "A.Grande.Família".
    let stem = SITE.replace_all(&stem, "");
    let stem = DOTTED_RELEASE.replace_all(&stem, "").trim().to_string();

    let mut guess = Guess {
        looks_like_anime: anime_hint,
        ..Default::default()
    };

    // [SubsPlease] à frente é grupo de fansub, não título.
    let without_group = if let Some(captures) = LEADING_GROUP.captures(&stem) {
        guess.release_group = Some(captures[1].to_string());
        guess.looks_like_anime = true;
        stem[captures.get(0).unwrap().end()..].to_string()
    } else {
        stem.clone()
    };

    // Heurística de scene release: se não há espaço nenhum, os pontos/underscores
    // SÃO os espaços. Se já há espaços, mexer nos pontos quebraria "Mr. Robot".
    let normalized = if without_group.contains(' ') {
        without_group.replace('_', " ")
    } else {
        without_group.replace('.', " ").replace('_', " ")
    };
    let normalized = SPACES.replace_all(&normalized, " ").trim().to_string();

    let mut cut = normalized.len();

    if let Some(c) = SEASON_EP.captures(&normalized) {
        guess.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        guess.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if let Some(c) = SEASON_EP_X.captures(&normalized) {
        guess.season = c.get(1).and_then(|m| m.as_str().parse().ok());
        guess.episode = c.get(2).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if let Some(c) = SEASON_EP_WORDS.captures(&normalized) {
        // Por extenso não é ambíguo em lugar nenhum: não precisa do contexto
        // da biblioteca.
        guess.season = c
            .get(1)
            .or_else(|| c.get(2))
            .and_then(|m| m.as_str().parse().ok());
        guess.episode = c.get(3).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if serial && EP_PREFIX.is_match(&normalized) {
        let c = EP_PREFIX.captures(&normalized).unwrap();
        guess.episode = c.get(1).and_then(|m| m.as_str().parse().ok());
        cut = cut.min(c.get(0).unwrap().start());
    } else if guess.looks_like_anime || serial {
        // Episódio absoluto exige contexto: sozinho, `Ocean's - 11` viraria
        // episódio 11. Com sinal de anime (grupo de fansub ou pasta "anime") ou
        // numa biblioteca declarada de episódios, o risco desaparece — ninguém
        // guarda filme numa biblioteca de séries.
        if let Some(c) = ABSOLUTE_EP.captures(&normalized) {
            guess.absolute_episode = c.get(1).and_then(|m| m.as_str().parse().ok());
            cut = cut.min(c.get(0).unwrap().start());
        }
    }

    // Índice na frente: `031 - Dia de Ventania`. Só depois das regras acima, e
    // só em biblioteca de episódios — num acervo de filmes isto destruiria
    // "007 - Cassino Royale".
    //
    // O que sobra depois do número é o título do EPISÓDIO, não o da série. Zerar
    // o corte faz o título ficar vazio de propósito, pra que a busca por pasta
    // (em `guess_from_path`) forneça o nome da série — que é o que o provider
    // precisa.
    let mut drop_title = false;
    if serial && guess.any_episode().is_none() {
        if let Some(c) = LEADING_INDEX.captures(&normalized) {
            guess.absolute_episode = c.get(1).and_then(|m| m.as_str().parse().ok());
            drop_title = true;
        }
    }

    // R54 — três formas que este acervo tem e o parser não lia. As três só
    // valem em biblioteca de série (`serial`), pela mesma razão que as de cima:
    // num acervo de filmes elas causariam estrago.
    //
    // A ordem importa. `TEMP_EP_TRACO` primeiro porque é a única que traz a
    // TEMPORADA junto — as outras duas dão um número absoluto, que o escopo
    // depois converte.
    if serial && guess.any_episode().is_none() && guess.season.is_none() {
        if let Some(c) = TEMP_EP_TRACO.captures(&normalized) {
            guess.season = c[1].parse().ok();
            guess.episode = c[2].parse().ok();
            drop_title = true;
        }
    }

    if serial && guess.any_episode().is_none() {
        let cru = normalized.trim();
        if let Some(c) = SO_O_NUMERO.captures(cru) {
            guess.absolute_episode = c[1].parse().ok();
            drop_title = true;
        } else if let Some(c) = EP_ENTRE_PARENTESES.captures(cru) {
            // Ano entre parênteses é ano, não episódio: `Filme (2019)` casa com
            // a mesma expressão, e é aqui que os dois se separam.
            if let Ok(v) = c[1].parse::<i32>() {
                if !(MIN_YEAR..=MAX_YEAR).contains(&v) {
                    guess.absolute_episode = Some(v);
                }
            }
        }
    }

    // Primeiro número de 4 dígitos que (a) não abre o nome — senão "2001 A Space
    // Odyssey" perde o título — e (b) é um ano plausível — senão "Blade Runner
    // 2049" vira "Blade Runner".
    for c in YEAR.captures_iter(&normalized) {
        let m = c.get(1).unwrap();
        let Ok(value) = m.as_str().parse::<i32>() else {
            continue;
        };
        if m.start() == 0 || !(MIN_YEAR..=MAX_YEAR).contains(&value) {
            continue;
        }
        guess.year = Some(value);
        cut = cut.min(c.get(0).unwrap().start());
        break;
    }

    if let Some(m) = JUNK.find(&normalized) {
        cut = cut.min(m.start());
    }

    let cut_title = &normalized[..cut];
    // Sobra de [1080p] / (hash) que não caiu em nenhuma regra acima.
    let cleaned = BRACKETED.replace_all(cut_title, " ");
    let cleaned = SPACES.replace_all(&cleaned, " ");
    // Separador solto nas duas pontas. O do INÍCIO passou a existir quando o
    // endereço de site começou a ser removido: `hidratorrent.com - Beavis`
    // vira ` - Beavis`, e o traço não é parte do nome de nada.
    let title = cleaned
        .trim()
        .trim_end_matches(|c: char| "-_.([ ".contains(c))
        .trim_start_matches(|c: char| "-_.)] ".contains(c))
        .trim()
        .to_string();

    guess.title = if drop_title {
        // Vazio de propósito: sinaliza "o nome da obra não está no arquivo".
        String::new()
    } else if title.is_empty() {
        normalized
    } else {
        title
    };
    guess
}

/// Um título só serve se sobrar letra. "S02E07", "12", "video" não servem.
fn is_informative(title: &str, serial: bool) -> bool {
    let trimmed = title.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if !trimmed.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    // Título que ainda carrega marcação de episódio não é nome de obra — é
    // nome de episódio, e a série está na pasta. `EP34 - Reflexos do Mal` cai
    // aqui pelo mesmo motivo que `S02E13 - Disposição Para Vitória`.
    //
    // `LEADING_INDEX` entra na lista porque um número na frente é a mesma
    // coisa: `18 - Drama Total Corrida Alucinante` é o nome do episódio 18,
    // não o da série. Sem ele, quando outra regra já tinha consumido a
    // numeração, o que sobrava passava por título e a pasta nunca era
    // consultada.
    if SEASON_EP.is_match(trimmed) || SEASON_EP_X.is_match(trimmed) {
        return false;
    }
    // Estas duas dependem do contexto da biblioteca, pelo mesmo motivo que as
    // regras que as produzem: num acervo de FILMES, `007 - Cassino Royale` é um
    // título legítimo e `EP` costuma ser disco de música. Só em biblioteca de
    // episódios elas significam "isto é nome de episódio, a série está na
    // pasta".
    if serial && (EP_PREFIX.is_match(trimmed) || LEADING_INDEX.is_match(trimmed)) {
        return false;
    }
    let lower = trimmed.to_lowercase();
    // **Palavra que descreve o arquivo, não a obra** (R71).
    //
    // `episode` já estava aqui; `episódio` não — e era o caso de **421
    // arquivos** deste acervo, o segundo maior grupo da fila de revisão:
    //
    //     /media2/TV Show/ALF/Episódio 2x25 - Varsity Drag (Web-DL).mkv
    //     /media2/TV Show/Liga da Justiça/Episódio 2x12 - A Better World.mkv
    //
    // O `SEASON_EP_X` consome o `2x25`, sobra "Episódio", e ele passava por
    // título de obra. O provider então devolvia o que tem essa palavra no nome
    // — *"Star & Sky Episódio Especial"*, *"The Witcher — Por Dentro dos
    // Episódios"* — e três palpites errados iam pra revisão humana. **Não era
    // escolha errada: era ausência de escolha**, e o trabalho não existia.
    //
    // Com a palavra aqui, o `guess_from_path` sobe um nível e acha ALF e a Liga
    // da Justiça, que é onde a informação sempre esteve.
    //
    // ⚠️ A lista é de palavras **inteiras**, e isso é o que a torna segura:
    // `matches!` compara a string toda, então *"Episódios de uma Guerra"*
    // continua sendo um título. Acentuada e sem acento porque o disco tem as
    // duas grafias, e `to_lowercase` preserva o acento.
    !matches!(
        lower.as_str(),
        "video"
            | "movie"
            | "film"
            | "filme"
            | "index"
            | "main"
            | "playback"
            | "episode"
            | "episódio"
            | "episodio"
            | "episódios"
            | "episodios"
            | "capítulo"
            | "capitulo"
            | "temporada"
            | "season"
            | "disco"
            | "disc"
    )
}

#[cfg(test)]
mod tests {

    /// **R71 — 421 arquivos cujo nome não diz de que série são.**
    ///
    /// Os três caminhos são reais, copiados do disco. `episode` estava na lista
    /// de palavras que não são título; `episódio`, não — e é a palavra que este
    /// acervo usa.
    #[test]
    fn episodio_nao_e_titulo_e_a_pasta_e() {
        let de = |caminho: &str| {
            super::guess_from_path(
                std::path::Path::new(caminho),
                std::path::Path::new("/media2/TV Show"),
                true,
            )
        };

        assert_eq!(
            de("/media2/TV Show/ALF/Episódio 2x25 - Varsity Drag (Web-DL do MAX-Brasil).mkv").title,
            "ALF"
        );
        assert_eq!(
            de("/media2/TV Show/Liga da Justiça/Episódio 2x12 - A Better World (2-2) - Web-DL.mkv")
                .title,
            "Liga da Justiça"
        );
        // E a numeração continua saindo do nome do arquivo.
        let g = de("/media2/TV Show/ALF/Episódio 2x25 - Varsity Drag (Web-DL do MAX-Brasil).mkv");
        assert_eq!(g.season, Some(2));
        assert_eq!(g.episode, Some(25));
    }

    /// **A convenção de pastas desta casa**, dita pelo dono em 20/08/2026:
    ///
    /// > *"Pasta com nome da série e dentro uma pasta para cada temporada ou
    /// > extras."*
    ///
    /// Ela vale pra série e pra vídeo de YouTube, e é a estrutura que o
    /// `guess_from_path` foi feito pra ler — mas até a R71 ela não bastava
    /// quando o arquivo dizia só "Episódio". Estes casos ficam escritos porque
    /// **a convenção é dado**: se alguém mexer no `SEASON_DIR` ou no
    /// `EXTRAS_DIR`, é aqui que quebra.
    #[test]
    fn a_estrutura_desta_casa() {
        let de = |caminho: &str| {
            super::guess_from_path(
                std::path::Path::new(caminho),
                std::path::Path::new("/media2/TV Show"),
                true,
            )
        };

        // Série / Temporada N / arquivo — dois níveis, e o do meio é pulado.
        let g = de("/media2/TV Show/Um Maluco no Pedaço/Temporada 6/Episódio 6x17 - Titulo.mkv");
        assert_eq!(g.title, "Um Maluco no Pedaço");
        assert_eq!(g.season, Some(6));
        assert_eq!(g.episode, Some(17));

        // A pasta de temporada com nome de release também é pulada como
        // temporada — mas ela **carrega o título**, e aí serve de fonte. É o
        // caso de *A Grande Família* neste acervo.
        let g = de("/media2/TV Show/A Grande Família/A.Grande.Família.S12.720p.WEB-DL/A.Grande.Família.S12E03.mp4");
        assert_eq!(g.title, "A Grande Família");
        assert_eq!(g.season, Some(12));

        // Série / Extras / arquivo, quando o arquivo **não** se nomeia: a
        // pasta de extras é pulada e a série vem de cima. "Especiais" é como
        // esta casa escreve, e o `SEASON_DIR` já a conhece.
        let g = de("/media2/TV Show/Liga da Justiça/Especiais/Episódio 1x01 - Bastidores.mkv");
        assert_eq!(g.title, "Liga da Justiça");

        // ⚠️ **E aqui está o limite de hoje, escrito de propósito.** Quando o
        // extra tem nome próprio, ele vira o título e a série não é
        // consultada: `ALF/Extras/Making Of.mkv` é a obra "Making Of", solta.
        //
        // Não é descuido do `guess_from_path` — é a regra dele: só sobe a
        // árvore quando o nome do arquivo não informa nada, e "Making Of"
        // informa. Ligar o extra à série pelo caminho é decisão de produto e
        // não de parser: são 941 arquivos sob pastas de extras neste acervo, e
        // **693 deles já estão `ignored`** — alguém decidiu, um a um, que eles
        // não entram na biblioteca. Mudar isto reabriria essa decisão.
        let g = de("/media2/TV Show/ALF/Extras/Making Of.mkv");
        assert_eq!(g.title, "Making Of");
    }

    /// ⚠️ A lista é de palavras **inteiras**. Um título que começa com a
    /// palavra não pode ser jogado fora junto.
    #[test]
    fn titulo_que_contem_a_palavra_continua_valendo() {
        let g = super::guess_from_path(
            std::path::Path::new("/tv/Episódios de uma Guerra/Episódio 1x01 - Piloto.mkv"),
            std::path::Path::new("/tv"),
            true,
        );
        assert_eq!(g.title, "Episódios de uma Guerra");
    }

    /// **R54 — os três formatos que este acervo tinha e o parser não lia.**
    ///
    /// Os nomes são reais, copiados do disco. Juntos eram 274 arquivos sem
    /// número de episódio nenhum — e sem episódio o scanner não cria
    /// `collection(série)`, então cada um virava um cartão solto na biblioteca.
    #[test]
    fn os_tres_formatos_do_acervo() {
        // `serial = true`: é a chave que libera estas regras, e ela vem do
        // `library.default_kind`. Fora de uma biblioteca de série, nenhuma vale.
        let em_serie = |nome: &str| {
            guess_from_path(
                &std::path::Path::new("/tv").join(nome),
                std::path::Path::new("/tv"),
                true,
            )
        };

        // Dr House: temporada e episódio colados por traço, no começo.
        let h = em_serie("08-21 720p (Aguentando) by KiLLerBeSama.mkv");
        assert_eq!((h.season, h.episode), (Some(8), Some(21)));

        // Pica-Pau: o absoluto entre parênteses, no fim.
        let p = em_serie("Pica-Pau.WEB.DUB-WWW.BLUDV.COM (125).mkv");
        assert_eq!(p.absolute_episode, Some(125));

        // Kenan e Kel: o nome é só o número.
        let k = em_serie("12.avi");
        assert_eq!(k.absolute_episode, Some(12));
    }

    /// **E o que essas três regras NÃO podem quebrar.**
    ///
    /// Cada uma tem um vizinho perigoso, e é ele que este teste guarda.
    #[test]
    fn os_tres_formatos_nao_comem_o_que_nao_e_deles() {
        let em_serie = |nome: &str| {
            guess_from_path(
                &std::path::Path::new("/tv").join(nome),
                std::path::Path::new("/tv"),
                true,
            )
        };

        // Ano entre parênteses é ano. Este é o vizinho do Pica-Pau, e sem o
        // corte de faixa ele viraria episódio 2019.
        let f = em_serie("Alguma Coisa (2019).mkv");
        assert_eq!(f.absolute_episode, None, "ano não é episódio");
        assert_eq!(f.year, Some(2019));

        // Quatro dígitos sozinhos são ano, não episódio — o vizinho do Kenan.
        let a = em_serie("2012.mkv");
        assert_eq!(a.absolute_episode, None);

        // Resolução com traço não é temporada — o vizinho do Dr House.
        let r = em_serie("1080-720p alguma coisa.mkv");
        assert_eq!(r.season, None, "1080-720p não é temporada 10");

        // E nada disso vale fora de biblioteca de série.
        let filme = guess_from_path(
            std::path::Path::new("/media/Movies/12.mkv"),
            std::path::Path::new("/media/Movies"),
            false,
        );
        assert_eq!(filme.absolute_episode, None, "num acervo de filmes, 12 é um título");
    }

    /// A pasta `1°Temp` do `Kenan e Kel`: o `°` é SINAL DE GRAU (U+00B0), e não
    /// o ordinal masculino `º` (U+00BA) que a expressão aceitava. Dois glifos
    /// que se parecem, e um deles fazia a temporada sumir.
    #[test]
    fn temporada_com_sinal_de_grau() {
        // Duas coisas quebravam aqui, e só uma era o glifo: "Temp" é abreviação,
        // e a expressão só conhecia "temporada" por extenso.
        assert!(SEASON_DIR.is_match("1°Temp"), "1°Temp é temporada 1");
        assert!(SEASON_DIR.is_match("2ª Temporada"), "e o ordinal continua valendo");
        assert!(SEASON_DIR.is_match("Temporada 3"), "e a forma por extenso também");
        assert!(!SEASON_DIR.is_match("Tempos Modernos"), "mas 'Tempos' não é temporada");
    }

    /// **R54 — o caso do `Arrested Development`, e o falso positivo que ele quase
    /// trouxe junto.**
    ///
    /// `S04RE01` é a temporada 4 **Remix**. O `R` no meio quebrava a expressão, e
    /// 22 arquivos ficavam sem episódio — o que os impedia de virar uma série na
    /// biblioteca, porque sem episódio não nasce `collection`.
    ///
    /// A segunda metade do teste é o motivo de o remendo ser `r?` e não
    /// `[a-z]{0,2}`: com duas letras livres, `S02 The 100` viraria temporada 2,
    /// episódio 100. "The 100" é uma série de verdade.
    #[test]
    fn remix_casa_e_the_100_nao() {
        let g = guess_from_filename(
            "Arrested Development (2003) - S04RE01 - Re Cap'n Bluth (1080p NF WEB-DL x265).mkv",
        );
        assert_eq!(g.season, Some(4), "a temporada do remix");
        assert_eq!(g.episode, Some(1), "o episódio do remix");

        // E o formato normal não mudou.
        let n = guess_from_filename("Breaking Bad - S02E07 - Negro y Azul.mkv");
        assert_eq!((n.season, n.episode), (Some(2), Some(7)));

        // O falso positivo que a versão genérica criaria.
        let t = guess_from_filename("S02 The 100 1080p.mkv");
        assert_ne!(
            t.episode,
            Some(100),
            "duas letras livres fariam 'The 100' virar episódio 100"
        );
    }
    use super::*;
    use std::path::PathBuf;

    fn g(s: &str) -> Guess {
        guess_from_filename(s)
    }

    /// `library.default_kind`: as regras de índice-na-frente e episódio absoluto
    /// sem sinal de anime só valem em biblioteca de episódios.
    const SERIES: bool = true;
    const FILMES: bool = false;

    /// 1.542 obras deste acervo têm pelo menos um sósia no título.
    #[test]
    fn sosia_de_caractere_proibido_volta_a_ser_o_caractere() {
        assert_eq!(
            g("Happy Tree Friends： Still Alive - Just be Claus.mkv").title,
            "Happy Tree Friends: Still Alive - Just be Claus"
        );
        assert_eq!(g("Snow What？ That's What.mp4").title, "Snow What? That's What");
        assert_eq!(g("S06 - Q&A w⧸Raphael Bob-Waksberg.mkv").title, "S06 - Q&A w/Raphael Bob-Waksberg");
    }

    /// Título sem sósia nenhum não paga nada: a função sai na primeira linha.
    #[test]
    fn titulo_limpo_nao_e_tocado_pela_restauracao() {
        assert_eq!(restaura_o_que_o_disco_proibiu("Blade Runner 2049"), "Blade Runner 2049");
    }

    #[test]
    fn scene_release_de_filme() {
        let r = g("Blade.Runner.2049.2017.1080p.BluRay.x265-RARBG.mkv");
        assert_eq!(r.title, "Blade Runner 2049");
        assert_eq!(r.year, Some(2017));
        assert_eq!(r.episode, None);
    }

    #[test]
    fn episodio_sxxexx() {
        let r = g("Severance.S02E07.1080p.WEB-DL.mkv");
        assert_eq!(r.title, "Severance");
        assert_eq!(r.season, Some(2));
        assert_eq!(r.episode, Some(7));
    }

    #[test]
    fn episodio_formato_x() {
        let r = g("Arcane 1x03 - The Base Violence Necessary for Change.mkv");
        assert_eq!(r.title, "Arcane");
        assert_eq!(r.season, Some(1));
        assert_eq!(r.episode, Some(3));
    }

    #[test]
    fn nome_ja_limpo_com_espacos() {
        let r = g("Cidade de Deus (2002).mp4");
        assert_eq!(r.title, "Cidade de Deus");
        assert_eq!(r.year, Some(2002));
    }

    #[test]
    fn ano_no_inicio_faz_parte_do_titulo() {
        let r = g("2001 A Space Odyssey 1968 1080p.mkv");
        assert_eq!(r.title, "2001 A Space Odyssey");
        assert_eq!(r.year, Some(1968));
    }

    #[test]
    fn sem_metadata_nenhuma() {
        let r = g("gravacao aleatoria.mov");
        assert_eq!(r.title, "gravacao aleatoria");
        assert_eq!(r.year, None);
    }

    // --- anime ---

    #[test]
    fn anime_com_grupo_e_episodio_absoluto() {
        let r = g("[SubsPlease] Sousou no Frieren - 12 [1080p][A1B2C3D4].mkv");
        assert_eq!(r.title, "Sousou no Frieren");
        assert_eq!(r.absolute_episode, Some(12));
        assert_eq!(r.season, None);
        assert_eq!(r.release_group.as_deref(), Some("SubsPlease"));
        assert!(r.looks_like_anime);
    }

    #[test]
    fn anime_com_versao_de_release() {
        let r = g("[Erai-raws] Bocchi the Rock! - 08v2 [1080p].mkv");
        assert_eq!(r.absolute_episode, Some(8));
        assert!(r.title.starts_with("Bocchi the Rock"));
    }

    #[test]
    fn traco_com_numero_sem_sinal_de_anime_nao_vira_episodio() {
        // sem [grupo] e sem pasta "anime", isso é só um título
        let r = g("Ocean's - 11.mkv");
        assert_eq!(r.absolute_episode, None);
    }

    // --- contexto de diretório ---

    #[test]
    fn titulo_vem_da_pasta_pulando_a_temporada() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Severance/Season 2/S02E07.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.title, "Severance");
        assert_eq!(r.season, Some(2));
        assert_eq!(r.episode, Some(7));
    }

    #[test]
    fn temporada_vem_da_pasta_quando_o_arquivo_nao_diz() {
        // O layout mais comum: o arquivo é só um número, a temporada está na
        // pasta. Sem isto a obra nasce sem `season_number`.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Friends/Season 3/01.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.title, "Friends");
        assert_eq!(r.season, Some(3));
    }

    #[test]
    fn pasta_de_especiais_e_temporada_zero() {
        // Temporada 0 é a convenção do TMDB pra especiais.
        let root = PathBuf::from("/media");
        for pasta in ["Specials", "Especiais"] {
            let path = PathBuf::from(format!("/media/Dark/{pasta}/making-of.mkv"));
            let r = guess_from_path(&path, &root, SERIES);
            assert_eq!(r.season, Some(0), "pasta {pasta}");
        }

        // E a pasta de especiais é pulada na busca do título, como qualquer
        // pasta de temporada — senão "Specials" viraria o nome da obra.
        let path = PathBuf::from("/media/Dark/Specials/03.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.title, "Dark");
        assert_eq!(r.season, Some(0));
    }

    #[test]
    fn notacao_luso_brasileira_txxexx() {
        let root = PathBuf::from("/media");
        for (arquivo, s, e) in [
            ("Heartland T07E08.mkv", 7, 8),
            ("Todo.Mundo.Odeia.o.Chris.T02.E16.avi", 2, 16),
            ("Os Simpsons S08 E 02.avi", 8, 2),
        ] {
            let path = PathBuf::from(format!("/media/Serie/{arquivo}"));
            let r = guess_from_path(&path, &root, SERIES);
            assert_eq!(r.season, Some(s), "{arquivo}");
            assert_eq!(r.episode, Some(e), "{arquivo}");
        }
    }

    #[test]
    fn temporada_e_episodio_por_extenso() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Drama Total/3ª Temporada - Episódio 04.mp4");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.season, Some(3));
        assert_eq!(r.episode, Some(4));
    }

    #[test]
    fn prefixo_ep_so_em_biblioteca_de_serie() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Power Rangers/EP13 - Um Raio Azul.mp4");

        let serie = guess_from_path(&path, &root, SERIES);
        assert_eq!(serie.episode, Some(13));

        // Numa biblioteca de filmes "EP" costuma ser disco de música.
        let filme = guess_from_path(&path, &root, FILMES);
        assert_eq!(filme.episode, None);
    }

    #[test]
    fn endereco_de_site_nao_entra_no_titulo() {
        // Carimbo de tracker. Sem removê-lo antes da normalização, o título
        // buscado no provider carrega o domínio e não casa com nada.
        let root = PathBuf::from("/media");
        for (arquivo, esperado) in [
            ("Pica-Pau.WEB.DUB-WWW.BLUDV.COM (11).mkv", "Pica-Pau"),
            ("hidratorrent.com - Beavis and Butt-Head.mkv", "Beavis and Butt-Head"),
        ] {
            let path = PathBuf::from(format!("/media/Serie/{arquivo}"));
            let r = guess_from_path(&path, &root, SERIES);
            assert_eq!(r.title, esperado, "{arquivo}");
        }
    }

    #[test]
    fn indice_na_frente_com_ponto() {
        // `094. Tom & Jerry - ...` é a mesma forma que `094 - ...`, com outro
        // separador. O espaço depois do ponto é obrigatório: sem ele,
        // `1.5 Something` viraria episódio 1.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Tom & Jerry/094. Tom and Cherie.avi");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.any_episode(), Some(94));
        assert_eq!(r.title, "Tom & Jerry");
    }

    #[test]
    fn episodio_por_extenso_sem_temporada() {
        // Forma comum em material dublado. O `\bep` seguido de dígito não casa
        // com "Episódio", porque depois do `ep` vem `i` — daí a palavra inteira
        // precisar estar no regex.
        let root = PathBuf::from("/media");
        for arquivo in [
            "Episódio 12 - Ruína e Redenção de Apu.mp4",
            "Episodio 7 - Alguma Coisa.avi",
            "Ep 22 - Outra.mkv",
        ] {
            let path = PathBuf::from(format!("/media/Os Simpsons/{arquivo}"));
            let r = guess_from_path(&path, &root, SERIES);
            assert!(r.any_episode().is_some(), "{arquivo} não achou episódio");
            // E o título cede pra pasta: o que sobra é o nome do EPISÓDIO.
            assert_eq!(r.title, "Os Simpsons", "{arquivo}");
        }
    }

    #[test]
    fn indice_na_frente_cede_o_titulo_pra_pasta() {
        // `031 - Dia de Ventania` traz o nome do EPISÓDIO. A série só existe na
        // pasta, então o parse abre mão do título de propósito.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Pica-Pau/031 - Dia de Ventania.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.any_episode(), Some(31));
        assert_eq!(r.title, "Pica-Pau");
    }

    #[test]
    fn indice_na_frente_nao_estraga_filme() {
        // A regra do índice destruiria "007 - Cassino Royale" num acervo de
        // filmes. Por isso ela é condicionada à biblioteca.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Filmes/007 - Cassino Royale.mkv");
        let r = guess_from_path(&path, &root, FILMES);
        assert_eq!(r.any_episode(), None);
        assert!(r.title.contains("007"), "título foi {:?}", r.title);
    }

    #[test]
    fn episodio_absoluto_sem_sinal_de_anime_em_biblioteca_de_serie() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Naruto Shippuuden/Naruto Shippuuden - 413.mkv");

        let serie = guess_from_path(&path, &root, SERIES);
        assert_eq!(serie.absolute_episode, Some(413));
        assert_eq!(serie.title, "Naruto Shippuuden");

        // Sem o contexto, "Ocean's - 11" viraria episódio: continua desligado.
        let filme = guess_from_path(&path, &root, FILMES);
        assert_eq!(filme.absolute_episode, None);
    }

    #[test]
    fn pasta_de_temporada_em_portugues_nao_vira_titulo() {
        // Formas reais do acervo. Sem isto o título da obra virava
        // "6 Temporada" e a série se perdia.
        let root = PathBuf::from("/media");
        for (pasta, esperado) in [
            ("6 Temporada", 6),
            ("2ª Temporada", 2),
            ("3a.Temporada", 3),
            ("2ª Temporada Completa", 2),
            ("Season 4 (BluRay)", 4),
            ("Season 6 (AMZN WEB-DL)", 6),
        ] {
            let path = PathBuf::from(format!("/media/Os Simpsons/{pasta}/ep.avi"));
            let r = guess_from_path(&path, &root, SERIES);
            assert_eq!(r.title, "Os Simpsons", "pasta {pasta:?}");
            assert_eq!(r.season, Some(esperado), "pasta {pasta:?}");
        }
    }

    #[test]
    fn pasta_de_extras_nao_vira_titulo() {
        // Sem isto, um episódio em `The Office/Featurettes/` passava a se
        // chamar "Featurettes" — pior que o nome do arquivo.
        let root = PathBuf::from("/media");
        // Nome de arquivo que não diz nada sozinho — é o que faz a busca subir
        // pela árvore. Com um nome informativo o fallback nem roda.
        for pasta in ["Featurettes", "Deleted Scenes", "Promos", "Making Of"] {
            let path = PathBuf::from(format!("/media/The Office/{pasta}/01.mkv"));
            let r = guess_from_path(&path, &root, SERIES);
            assert_eq!(r.title, "The Office", "pasta {pasta:?}");
            // Extra não é temporada 0 — não existe no provider, é outra coisa.
            assert_eq!(r.season, None, "pasta {pasta:?}");
        }
    }

    #[test]
    fn pasta_que_comeca_com_o_nome_da_serie_nao_e_pasta_de_temporada() {
        // "Breaking Bad (2008) Season 1-5" MENCIONA temporada, mas o nome da
        // série vem antes — descartá-la jogaria fora a única informação útil.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Breaking Bad (2008) Season 1-5/Season 3/ep.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert!(
            r.title.starts_with("Breaking Bad"),
            "título foi {:?}",
            r.title
        );
        assert_eq!(r.season, Some(3));
    }

    #[test]
    fn temporada_do_arquivo_ganha_da_pasta() {
        // A pasta é fallback, não sobrescrita: se o arquivo diz S05E02, é isso
        // que vale mesmo dentro de uma pasta com outro número.
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Lost/Season 1/Lost.S05E02.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert_eq!(r.season, Some(5));
        assert_eq!(r.episode, Some(2));
    }

    #[test]
    fn pasta_com_ano_completa_o_arquivo() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Cidade de Deus (2002)/video.mkv");
        let r = guess_from_path(&path, &root, FILMES);
        assert_eq!(r.title, "Cidade de Deus");
        assert_eq!(r.year, Some(2002));
    }

    #[test]
    fn nome_de_arquivo_bom_ignora_a_pasta() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Baixados/Blade.Runner.2049.2017.1080p.mkv");
        let r = guess_from_path(&path, &root, FILMES);
        assert_eq!(r.title, "Blade Runner 2049");
    }

    #[test]
    fn pasta_anime_liga_a_heuristica() {
        let root = PathBuf::from("/media");
        let path = PathBuf::from("/media/Anime/Frieren/Frieren - 04.mkv");
        let r = guess_from_path(&path, &root, SERIES);
        assert!(r.looks_like_anime);
        assert_eq!(r.absolute_episode, Some(4));
    }
}
