mod common;

use memory_mcp::memory::episode_context_store::EpisodeContextStore;
use memory_mcp::models::{AssembleContextRequest, IngestRequest};
use memory_mcp::service::memory_container_shims::{
    memory_capabilities_assemble_context::AssembleContextCapability,
    memory_capabilities_extract::ExtractCapability, memory_capabilities_ingest::IngestCapability,
};

struct TurnTimes<'a> {
    reference: &'a str,
    ingested: Option<&'a str>,
}

async fn ingest_turn(
    service: &memory_mcp::service::MemoryService,
    source_id: &str,
    group: &str,
    position: u64,
    content: &str,
) {
    ingest_tagged_turn(service, source_id, group, position, content, vec![]).await;
}

async fn ingest_tagged_turn(
    service: &memory_mcp::service::MemoryService,
    source_id: &str,
    group: &str,
    position: u64,
    content: &str,
    policy_tags: Vec<&str>,
) {
    ingest_tagged_turn_at(
        service,
        source_id,
        group,
        position,
        content,
        policy_tags,
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;
}

async fn ingest_tagged_turn_at(
    service: &memory_mcp::service::MemoryService,
    source_id: &str,
    group: &str,
    position: u64,
    content: &str,
    policy_tags: Vec<&str>,
    times: TurnTimes<'_>,
) -> String {
    let request: IngestRequest = serde_json::from_value(serde_json::json!({
        "source_type": "conversation",
        "source_id": source_id,
        "content": content,
        "t_ref": times.reference,
        "t_ingested": times.ingested,
        "policy_tags": policy_tags,
        "source_context": {"group": group, "position": position},
    }))
    .expect("a conversation turn is a valid ingest request");
    let episode_id = IngestCapability::ingest_from_service(service, request, None)
        .await
        .expect("turn should ingest");
    ExtractCapability::extract_from_service(service, &episode_id, None, None)
        .await
        .expect("turn should extract");
    episode_id
}

fn query(access: Option<memory_mcp::models::AccessPayload>) -> AssembleContextRequest {
    query_text("Which team did Mira sign with on 21 May 2023?", access)
}

fn query_text(
    query: &str,
    access: Option<memory_mcp::models::AccessPayload>,
) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.into(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access,
        compact: false,
    }
}

#[tokio::test]
async fn named_answer_in_adjacent_turn_survives_context_assembly() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:meeting-a:1",
        "chat:meeting-a",
        1,
        "Mira: I signed with a new sports team today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-a:2",
        "chat:meeting-a",
        2,
        "Owen: Which team did you sign with?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-a:3",
        "chat:meeting-a",
        3,
        "Mira: The Minnesota Wolves! I am excited to play for them.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(&tm.service, query(None))
        .await
        .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Minnesota Wolves")
                && item.fact_id.starts_with("fact:")),
        "the answer should be returned as an extracted fact with provenance, not a raw episode: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Which team did you sign with?")),
        "a source question may guide retrieval but is not answer evidence: {items:#?}"
    );
}

#[tokio::test]
async fn named_subject_source_question_recovers_its_adjacent_answer() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:named-source-question:1",
        "chat:named-source-question",
        1,
        "Mira: I signed with a new sports team today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:named-source-question:2",
        "chat:named-source-question",
        2,
        "Owen: Which team did Mira sign with?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:named-source-question:3",
        "chat:named-source-question",
        3,
        "Mira: The Minnesota Wolves!",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which team did Mira sign with?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Minnesota Wolves")),
        "the named-subject source question should retain its adjacent answer: {items:#?}"
    );
}

#[tokio::test]
async fn typed_language_query_rejects_topic_only_and_unrelated_entity_evidence() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:language-slot:1",
        "chat:language-slot",
        1,
        "James: Programming is an awesome skill. I tried it out one in college and now it`s all my life. Good luck in the class! Do you have any coding experience?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:language-slot:2",
        "chat:language-slot",
        2,
        "James: I've worked with Python and C++. I've built a website and also created some game mods.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:language-slot:3",
        "chat:language-slot",
        3,
        "James: It would be cool! For example, we could write some kind of application for dogs. By the way, my dogs.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What programming languages has James worked with?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Python and C++")),
        "the direct language assertion should remain available: {items:#?}"
    );
    assert!(
        items.iter().all(|item| {
            !item.content.contains("Programming is an awesome skill")
                && !item.content.contains("my dogs")
        }),
        "topic-only and entity-only evidence must not answer a typed language slot: {items:#?}"
    );
}

#[tokio::test]
async fn pet_slot_keeps_owned_snakes_and_rejects_an_anaphoric_pet_comment() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:pet-slot:1",
        "chat:pet-slot",
        1,
        "Jolene: I want to show you one of my snakes! They always calm me down and make me happy. This is Susie.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:pet-slot:2",
        "chat:pet-slot",
        2,
        "Deborah: Having a pet totally brightens up your life. It's great that it brings you comfort. Do you have any fun moments with your pet that you'd like to share?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:pet-slot:3",
        "chat:pet-slot",
        3,
        "Jolene: I was playing video games and my pet just slinked out of her cage and coiled up next to me - it was too funny! My second snake Seraphim did it. Look at her sly eyes!",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:pet-slot:4",
        "chat:pet-slot",
        4,
        "Deborah: Cool, Jolene! Pets bring so much happiness!",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:pet-slot:5",
        "chat:pet-slot",
        5,
        "Jolene: They are very unusual pets! Here's me and my partner gaming last week - it's so fun. We played the game Detroit on the console.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What pets does Jolene have?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("This is Susie")),
        "the first owned snake should remain available: {items:#?}"
    );
    assert!(
        items.iter().any(|item| item.content.contains("Seraphim")),
        "the second owned snake should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("They are very unusual pets")),
        "a bare anaphoric comment about pets is not ownership evidence: {items:#?}"
    );
}

#[tokio::test]
async fn unlisted_programming_language_value_is_not_a_contradiction() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:go-language:1",
        "chat:go-language",
        1,
        "Mira: I use Go to build and maintain the service API backend.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "What programming language does Mira use for the API backend?",
            None,
        ),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("use Go")),
        "a relevant answer with a valid but unlisted language value must survive normal retrieval: {items:#?}"
    );
}

#[tokio::test]
async fn learned_programming_languages_are_supported_answer_relations() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:learned-languages:1",
        "chat:learned-languages",
        1,
        "Mira: I learned the programming languages Go and Julia while building tools for my first engineering job.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which programming languages did Mira learn?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item
            .content
            .contains("learned the programming languages Go and Julia")),
        "a source-grounded learned-language answer must survive retrieval: {items:#?}"
    );
}

#[tokio::test]
async fn explicitly_named_pet_owner_matches_the_requested_subject() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:named-pet-owner:question",
        "chat:named-pet-owner",
        1,
        "Owen: What pets does Mira have?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:named-pet-owner:answer",
        "chat:named-pet-owner",
        2,
        "Nadia: Mira has two pet cats named Rumi and Pixel.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What pets does Mira have?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Rumi and Pixel")),
        "an explicit ownership assertion for the requested subject must be returned: {items:#?}"
    );
}

#[tokio::test]
async fn another_persons_pet_is_not_attributed_to_the_speaker() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:owned-dog:1",
        "chat:owned-dog",
        1,
        "Mira: I own a dog named Nala.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:brothers-cat:1",
        "chat:brothers-cat",
        1,
        "Mira: My brother has a cat named Basil; I do not own a cat myself, but I visit when he is out of town.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What pets does Mira own?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("Nala")),
        "the speaker's directly owned pet must remain retrievable: {items:#?}"
    );
    assert!(
        items.iter().all(|item| !item.content.contains("Basil")),
        "a family member's pet must not be treated as the speaker's pet: {items:#?}"
    );
}

#[tokio::test]
async fn possessive_source_question_matches_the_named_subjects_answer_slot() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:favorite-language:1",
        "chat:favorite-language",
        1,
        "Mira: I use several programming languages every day.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:favorite-language:2",
        "chat:favorite-language",
        2,
        "Owen: What is your favorite programming language?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:favorite-language:3",
        "chat:favorite-language",
        3,
        "Mira: Rust.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What is Mira's favorite programming language?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| {
            item.content.contains("Rust")
                && item
                    .provenance
                    .get("retrieval_admission")
                    .and_then(|admission| admission.get("basis"))
                    .and_then(serde_json::Value::as_str)
                    == Some("qualified_source_episode")
        }),
        "the source question's possessive pronoun should resolve to its bare adjacent answer: {items:#?}"
    );
}

#[tokio::test]
async fn adjacent_answer_with_a_conflicting_task_reference_is_not_admitted() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:task-reference:1",
        "chat:task-reference",
        1,
        "Mira: I use programming languages for task OPS-42.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:task-reference:2",
        "chat:task-reference",
        2,
        "Owen: What programming languages do you use for task OPS-42?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:task-reference:3",
        "chat:task-reference",
        3,
        "Mira: I use Rust for task OPS-99, not OPS-42.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "What programming languages does Mira use for task OPS-42?",
            None,
        ),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().all(|item| !item.content.contains("Rust")),
        "an answer explicitly scoped to another task must not enter the context: {items:#?}"
    );
}

#[tokio::test]
async fn a_positive_task_alternative_survives_an_excluded_alternative() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:reference-alternatives:1",
        "chat:reference-alternatives",
        1,
        "Mira: I use Rust for task OPS-99, not OPS-42.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "What programming languages does Mira use for task OPS-42 or OPS-99?",
            None,
        ),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("Rust")),
        "a positively supported requested alternative should survive an explicit exclusion of another alternative: {items:#?}"
    );
}

#[tokio::test]
async fn adjacent_answer_with_a_different_task_id_is_not_admitted() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:task-family:1",
        "chat:task-family",
        1,
        "Mira: I use programming languages for task OPS-42.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:task-family:2",
        "chat:task-family",
        2,
        "Owen: What programming languages do you use for task OPS-42?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:task-family:3",
        "chat:task-family",
        3,
        "Mira: I use Rust for task OPS-99.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "What programming languages does Mira use for task OPS-42?",
            None,
        ),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().all(|item| !item.content.contains("Rust")),
        "an answer scoped to another task in the same task family must not enter the context: {items:#?}"
    );
}

#[tokio::test]
async fn adjacent_answer_with_a_conflicting_predicate_is_not_admitted() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:predicate:1",
        "chat:predicate",
        1,
        "Mira: I enjoy movies.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:predicate:2",
        "chat:predicate",
        2,
        "Owen: What types of movies do you hate?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:predicate:3",
        "chat:predicate",
        3,
        "Mira: I hate horror movies.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What types of movies does Mira enjoy?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().all(|item| !item.content.contains("horror")),
        "an answer to the opposite predicate must not enter the context: {items:#?}"
    );
}

#[tokio::test]
async fn adjacent_answer_with_a_different_language_relation_is_not_admitted() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:language-relation:1",
        "chat:language-relation",
        1,
        "Mira: I use programming languages.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:language-relation:2",
        "chat:language-relation",
        2,
        "Owen: Which languages can you speak?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:language-relation:3",
        "chat:language-relation",
        3,
        "Mira: French and Spanish.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What programming languages does Mira use?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("French") && !item.content.contains("Spanish")),
        "an answer about spoken languages must not answer which programming languages Mira uses: {items:#?}"
    );
}

#[tokio::test]
async fn matching_where_question_recovers_the_answer_from_a_family_trip() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:trip:1",
        "chat:trip",
        1,
        "Evan: I just got back from a road trip with my family.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:trip:2",
        "chat:trip",
        2,
        "Sam: Where'd you go, by the way?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:trip:3",
        "chat:trip",
        3,
        "Evan: We went to the Rockies.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Where has Evan been on roadtrips with his family?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("Rockies")),
        "a matching where-question should recover the answer from the same speaker: {items:#?}"
    );
}

#[tokio::test]
async fn an_old_to_new_vehicle_transition_recovers_the_prior_answer_turn() {
    let tm = common::TestMemory::new(false).await;
    for (position, content) in [
        (
            1,
            "Sam: Hey Evan, good to see you! What's new since we last met?",
        ),
        (
            2,
            "Evan: I just got back from a trip with my family in my new Prius.",
        ),
        (
            3,
            "Sam: What happened to the old one? Where'd you go, by the way?",
        ),
        (
            4,
            "Evan: My old Prius broke down, so I decided to get it repaired and sell it.",
        ),
    ] {
        ingest_turn(
            &tm.service,
            &format!("chat:vehicle:{position}"),
            "chat:vehicle",
            position,
            content,
        )
        .await;
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "What type of car did Evan get after his old Prius broke down?",
            None,
        ),
    )
    .await
    .expect("the vehicle context should assemble");

    assert!(
        items.iter().any(|item| {
            item.content.contains("new Prius")
                && item
                    .provenance
                    .get("retrieval_admission")
                    .and_then(|admission| admission.get("source_context_relation"))
                    .and_then(serde_json::Value::as_str)
                    == Some("query_related_antecedent")
        }),
        "the earlier new-car answer should be linked to the requested old/new transition: {items:#?}"
    );
}

#[tokio::test]
async fn a_shared_pet_topic_recovers_the_same_speakers_answer() {
    let tm = common::TestMemory::new(false).await;
    for (position, content) in [
        (1, "Jolene: My pets are two parrots named Sunny and Piper."),
        (2, "Deborah: Do you have any fun moments with your pet?"),
        (
            3,
            "Jolene: My parrots sing along when I play music in the morning.",
        ),
    ] {
        ingest_turn(
            &tm.service,
            &format!("chat:pets:{position}"),
            "chat:pets",
            position,
            content,
        )
        .await;
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which pets does Jolene have?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("parrot")),
        "a single shared pet topic should recover the answer from the same speaker: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Do you have any fun moments")),
        "the source question is context, not answer evidence: {items:#?}"
    );
}

#[tokio::test]
async fn dated_team_question_recovers_its_adjacent_answer() {
    let tm = common::TestMemory::new(false).await;
    for (position, content) in [
        (
            3,
            "John: That's great! I just signed with a new team - excited for the season!",
        ),
        (
            4,
            "Tim: Woohoo! Congrats on the new team. Which team did you sign with?",
        ),
        (
            5,
            "John: The Minnesota Wolves! I can't wait to play with them!",
        ),
    ] {
        ingest_turn(
            &tm.service,
            &format!("chat:team:{position}"),
            "chat:team",
            position,
            content,
        )
        .await;
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which team did John sign with on 21 May, 2023?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| {
            item.content.contains("Minnesota Wolves")
                && item
                    .provenance
                    .get("retrieval_admission")
                    .and_then(|admission| admission.get("basis"))
                    .and_then(serde_json::Value::as_str)
                    == Some("qualified_source_episode")
        }),
        "the dated question should resolve to its adjacent answer: {items:#?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.content.contains("signed with a new team")),
        "the same-speaker statement that establishes the question's subject should remain available as supporting evidence: {items:#?}"
    );
}

#[tokio::test]
async fn anaphoric_follow_up_answer_stays_linked_to_its_query_relevant_assertion() {
    let tm = common::TestMemory::new(false).await;
    let mut response_episode_id = String::new();
    for (position, content) in [
        (1, "Calvin: I just got a new car and it is amazing."),
        (2, "Dave: Congratulations on the new car!"),
        (3, "Calvin: It is a smooth and powerful ride."),
        (
            4,
            "Dave: Owning a car like this must feel amazing. Is this your first one? What's it called?",
        ),
        (
            5,
            "Calvin: I finally got myself this beauty. Here's a pic from yesterday when I took it for a ride.",
        ),
    ] {
        let episode_id = ingest_tagged_turn_at(
            &tm.service,
            &format!("chat:car-purchase:{position}"),
            "chat:car-purchase",
            position,
            content,
            vec![],
            TurnTimes {
                reference: "2023-05-21T12:00:00Z",
                ingested: None,
            },
        )
        .await;
        if position == 5 {
            response_episode_id = episode_id;
        }
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which new car did Calvin get on May 21, 2023?", None),
    )
    .await
    .expect("context should assemble");

    let response = items
        .iter()
        .find(|item| item.source_episode == response_episode_id)
        .unwrap_or_else(|| {
            panic!(
                "the same-speaker answer to the anaphoric follow-up should be retained: {items:#?}"
            )
        });
    assert_eq!(
        response.provenance["retrieval_admission"]["source_context_relation"],
        "anaphoric_follow_up_answer"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("What's it called?")),
        "the supporting question is context, not answer evidence: {items:#?}"
    );
}

#[tokio::test]
async fn anaphoric_follow_up_does_not_add_a_related_response_that_cannot_answer_the_query() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:dog-year:1",
        "chat:dog-year",
        1,
        "Mira: I adopted my first dog Luna in 2022. She loves the dog park and plays there every week.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:dog-year:2",
        "chat:dog-year",
        2,
        "Owen: Is this dog enjoying the dog park?",
    )
    .await;
    let response_episode_id = ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-year:3",
        "chat:dog-year",
        3,
        "Mira: Yes, the dog park has a great trail for her.",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What year did Mira adopt her first dog?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("2022")),
        "the requested adoption year should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.source_episode != response_episode_id),
        "a same-speaker anaphoric reply is not evidence for the requested year: {items:#?}"
    );
}

#[tokio::test]
async fn an_unrelated_duration_question_does_not_bridge_into_a_dog_adoption_answer() {
    let tm = common::TestMemory::new(false).await;
    ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-year-unrelated:1",
        "chat:dog-year-unrelated",
        1,
        "Mira: I adopted my first dog Luna in 2022, two years ago.",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-year-unrelated:2",
        "chat:dog-year-unrelated",
        2,
        "Owen: How long have you had this car?",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;
    let unrelated_response = ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-year-unrelated:3",
        "chat:dog-year-unrelated",
        3,
        "Mira: Three weeks.",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What year did Mira adopt each dog?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("first dog Luna")),
        "the requested adoption context should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.source_episode != unrelated_response),
        "an adjacent duration answer to a different question must be excluded: {items:#?}"
    );
}

#[tokio::test]
async fn a_shared_pronoun_does_not_ground_an_explicitly_different_duration_object() {
    let tm = common::TestMemory::new(false).await;
    ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-car-duration:1",
        "chat:dog-car-duration",
        1,
        "Mira: I adopted my first dog Luna in 2022, two years ago. It is a lovely dog.",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-car-duration:2",
        "chat:dog-car-duration",
        2,
        "Owen: How long have you been driving this car, and do you like it?",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;
    let unrelated_response = ingest_tagged_turn_at(
        &tm.service,
        "chat:dog-car-duration:3",
        "chat:dog-car-duration",
        3,
        "Mira: Three weeks.",
        vec![],
        TurnTimes {
            reference: "2024-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What year did Mira adopt each dog?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Luna in 2022")),
        "the requested adoption evidence should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.source_episode != unrelated_response),
        "the response to an explicit car question must not cross into dog evidence: {items:#?}"
    );
}

#[tokio::test]
async fn anaphoric_car_follow_up_requires_the_response_to_assert_the_requested_purchase() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:car-praise:1",
        "chat:car-praise",
        1,
        "Calvin: I just got a new car and it is amazing.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:car-praise:2",
        "chat:car-praise",
        2,
        "Dave: Congratulations on the new car! How's it driving?",
    )
    .await;
    let response_episode_id = ingest_tagged_turn_at(
        &tm.service,
        "chat:car-praise:3",
        "chat:car-praise",
        3,
        "Calvin: It is a smooth and powerful ride.",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: None,
        },
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which new car did Calvin get on May 21, 2023?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("new car")),
        "the direct purchase assertion should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.source_episode != response_episode_id),
        "a related driving reply does not assert which car was acquired: {items:#?}"
    );
}

#[tokio::test]
async fn a_following_question_does_not_match_topic_words_from_prior_sentences() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:job:1",
        "chat:job",
        1,
        "Gina: Hey Jon! Good to see you. What's up? Anything new?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:job:2",
        "chat:job",
        2,
        "Jon: Lost my job as a banker yesterday, so I'm gonna take a shot at starting my own business.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:job:3",
        "chat:job",
        3,
        "Gina: Sorry about your job Jon, but starting your own business sounds awesome! Unfortunately, I also lost my job at Door Dash this month. What business are you thinking of?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:job:4",
        "chat:job",
        4,
        "Jon: I'm starting a dance studio because I'm passionate about dancing.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("When did Jon lose his job as a banker?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("banker yesterday")),
        "the direct answer to the when query should be retained: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("dance studio")),
        "the answer to the unrelated follow-up question must not be expanded into the result: {items:#?}"
    );
}

#[tokio::test]
async fn answer_replaces_a_question_only_turn_in_context_assembly() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:meeting-b:1",
        "chat:meeting-b",
        1,
        "Mira: Practice went well today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-b:2",
        "chat:meeting-b",
        2,
        "Owen: Which team did you sign with?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-b:3",
        "chat:meeting-b",
        3,
        "Mira: The Minnesota Wolves! I am excited to play for them.",
    )
    .await;

    let mut request = query(None);
    request.budget = 1;
    let items = AssembleContextCapability::assemble_context_from_service(&tm.service, request)
        .await
        .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Minnesota Wolves")),
        "the answer to the selected question turn should be returned with its own provenance: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Which team did you sign with?")),
        "a question-only turn must not be returned as evidence once its answer is available: {items:#?}"
    );
    assert!(items.len() <= 1, "answer replacement must honor the budget");
}

#[tokio::test]
async fn answer_replaces_its_matching_question_when_the_context_budget_allows_both() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:meeting-c:1",
        "chat:meeting-c",
        1,
        "Mira: I signed with a new sports team today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-c:2",
        "chat:meeting-c",
        2,
        "Owen: Which team did you sign with?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:meeting-c:3",
        "chat:meeting-c",
        3,
        "Mira: The Minnesota Wolves! I am excited to play for them.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(&tm.service, query(None))
        .await
        .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Minnesota Wolves")),
        "return the answer that resolves the selected question: {items:#?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Which team did you sign with?")),
        "a question-only turn is not independent answer evidence: {items:#?}"
    );
}

#[tokio::test]
async fn answer_to_a_followup_question_retains_relevant_prior_answer() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "chat:languages:1",
        "chat:languages",
        1,
        "John: When did you begin your software studies?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:languages:2",
        "chat:languages",
        2,
        "James: I've worked with Python and C++ on programming projects.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:languages:3",
        "chat:languages",
        3,
        "John: What other programming languages have you worked with?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "chat:languages:4",
        "chat:languages",
        4,
        "James: I haven't worked with any other programming languages.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("What programming languages has James worked with?", None),
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("Python and C++")),
        "a follow-up answer should retain the prior relevant answer from the same speaker: {items:#?}"
    );
}

#[tokio::test]
async fn adjacent_answer_from_another_stream_is_not_returned() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "a:1",
        "a",
        1,
        "Mira: I signed with a new sports team today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "a:2",
        "a",
        2,
        "Owen: Which team did you sign with?",
    )
    .await;
    ingest_turn(&tm.service, "b:3", "b", 3, "Mira: The Phoenix Falcons!").await;

    let items = AssembleContextCapability::assemble_context_from_service(&tm.service, query(None))
        .await
        .expect("context should assemble");
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Phoenix Falcons"))
    );
}

#[tokio::test]
async fn denied_adjacent_answer_is_not_returned() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "a:1",
        "a",
        1,
        "Mira: I signed with a new sports team today.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "a:2",
        "a",
        2,
        "Owen: Which team did you sign with?",
    )
    .await;
    ingest_tagged_turn(
        &tm.service,
        "a:3",
        "a",
        3,
        "Mira: The Phoenix Falcons!",
        vec!["private"],
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query(Some(memory_mcp::models::AccessPayload {
            allowed_tags: Some(vec!["public".into()]),
            ..Default::default()
        })),
    )
    .await
    .expect("context should assemble");
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Phoenix Falcons"))
    );
}

#[tokio::test]
async fn answer_after_historical_cutoff_is_not_returned() {
    let tm = common::TestMemory::new(false).await;
    ingest_tagged_turn_at(
        &tm.service,
        "a:1",
        "a",
        1,
        "Mira: I signed with a new sports team today.",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-22T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "a:2",
        "a",
        2,
        "Owen: Which team did you sign with?",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-22T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "a:3",
        "a",
        3,
        "Mira: The Phoenix Falcons!",
        vec![],
        TurnTimes {
            reference: "2023-06-01T12:00:00Z",
            ingested: Some("2023-05-22T12:00:00Z"),
        },
    )
    .await;

    let mut request = query(None);
    request.as_of = Some("2023-05-25T00:00:00Z".parse().expect("valid cutoff"));
    let items = AssembleContextCapability::assemble_context_from_service(&tm.service, request)
        .await
        .expect("historical context should assemble");
    assert!(
        items
            .iter()
            .all(|item| !item.content.contains("Phoenix Falcons"))
    );
}

#[tokio::test]
async fn batch_candidate_context_uses_four_turn_radius_and_keeps_temporal_bounds() {
    let tm = common::TestMemory::new(false).await;
    let cutoff = "2023-05-22T12:00:00Z";
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:1",
        "stream-a",
        1,
        "Mira: prior turn one",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:2",
        "stream-a",
        2,
        "Mira: prior turn two",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    let anchor_id = ingest_tagged_turn_at(
        &tm.service,
        "stream-a:3",
        "stream-a",
        3,
        "Mira: focal answer",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:4",
        "stream-a",
        4,
        "Owen: immediate follow-up",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:5",
        "stream-a",
        5,
        "Owen: not yet ingested at the historical cutoff",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-23T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:6",
        "stream-a",
        6,
        "Owen: inside the four-turn neighborhood",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:7",
        "stream-a",
        7,
        "Owen: last included turn",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-a:8",
        "stream-a",
        8,
        "Owen: outside the four-turn neighborhood",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;
    ingest_tagged_turn_at(
        &tm.service,
        "stream-b:3",
        "stream-b",
        3,
        "Owen: same position in another stream",
        vec![],
        TurnTimes {
            reference: "2023-05-21T12:00:00Z",
            ingested: Some("2023-05-21T12:00:00Z"),
        },
    )
    .await;

    let store = EpisodeContextStore::new(tm.db_client.clone(), "org");
    let episodes = store
        .select_source_neighborhoods_for_episodes(&[anchor_id], cutoff, None, None)
        .await
        .expect("batch source context should load");
    let source_ids = episodes
        .iter()
        .filter_map(|episode| episode.get("source_id").and_then(serde_json::Value::as_str))
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(
        source_ids,
        [
            "stream-a:1",
            "stream-a:2",
            "stream-a:3",
            "stream-a:4",
            "stream-a:6",
            "stream-a:7",
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
    );
}

#[tokio::test]
async fn query_related_answer_keeps_an_earlier_same_speaker_fact_in_the_source_window() {
    let tm = common::TestMemory::new(false).await;
    for (position, content) in [
        (1, "Audrey: I got new collars and tags for my dogs."),
        (2, "Andrew: Those collars look great."),
        (3, "Audrey: Thanks, the dogs love them."),
        (
            4,
            "Andrew: What are their names and how long have you had them?",
        ),
        (
            5,
            "Audrey: I've had them for three years. Their names are Pepper, Precious and Panda.",
        ),
    ] {
        ingest_turn(
            &tm.service,
            &format!("chat:dogs:{position}"),
            "chat:dogs",
            position,
            content,
        )
        .await;
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("Which year did Audrey adopt her first three dogs?", None),
    )
    .await
    .expect("the dog adoption context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("three years")),
        "the answer turn should be returned: {items:#?}"
    );
    assert!(
        items.iter().any(|item| {
            item.content.contains("collars and tags for my dogs")
                && item
                    .provenance
                    .get("retrieval_admission")
                    .and_then(|admission| admission.get("source_context_relation"))
                    .and_then(serde_json::Value::as_str)
                    == Some("query_related_antecedent")
        }),
        "the earlier dog turn should be admitted through its source relation, not incidental lexical retrieval: {items:#?}"
    );
}

#[tokio::test]
async fn shared_here_statement_retains_both_speakers_source_grounded_relief() {
    let tm = common::TestMemory::new(false).await;
    ingest_turn(
        &tm.service,
        "locomo:conv-30:D1:6",
        "locomo:conv-30:session_1",
        6,
        "Jon: I've been into dancing since I was a kid and it's been my passion and escape. I wanna start a dance studio so I can teach others the joy that dancing brings me.",
    )
    .await;
    ingest_turn(
        &tm.service,
        "locomo:conv-30:D1:7",
        "locomo:conv-30:session_1",
        7,
        "Gina: Wow Jon, same here! Dance is pretty much my go-to for stress relief. Got any fave styles?",
    )
    .await;
    ingest_turn(
        &tm.service,
        "locomo:conv-30:D2:11",
        "locomo:conv-30:session_2",
        11,
        "Gina: Believe in yourself, Jon! The process may be tough, but you got this. Push through and it'll be worth it. Don't forget to take breaks and dance it out when you need to destress!",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text("How do Jon and Gina both like to destress?", None),
    )
    .await
    .expect("context should assemble from the two source-grounded self reports");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("my passion and escape")),
        "Jon's directly source-grounded turn should remain visible: {items:#?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.content.contains("my go-to for stress relief")),
        "Gina's same-here self report should remain visible: {items:#?}"
    );
}

#[tokio::test]
async fn original_dog_adoption_query_excludes_a_later_puppy_session() {
    let tm = common::TestMemory::new(false).await;
    for (group, position, content) in [
        (
            "locomo:conv-44:session_1",
            3,
            "Audrey: Congrats on the new job! So I got these new collars and tags for my dogs - so cute!",
        ),
        (
            "locomo:conv-44:session_1",
            6,
            "Andrew: Cute little guys! What are their names and how long have you had them?",
        ),
        (
            "locomo:conv-44:session_1",
            7,
            "Audrey: I've had them for 3 years! Their names are Pepper, Precious and Panda. I can't live without my little ones!",
        ),
        (
            "locomo:conv-44:session_2",
            1,
            "Audrey: Hey Andrew, I got a surprise for you! We adopted another puppy called Pixie. She's SO cute! Isn't she just the cutest?",
        ),
        (
            "locomo:conv-44:session_2",
            2,
            "Andrew: That's awesome! Pixie is so cute! Did you ever check out that hiking spot we talked about last time?",
        ),
        (
            "locomo:conv-44:session_2",
            3,
            "Audrey: Thanks! I know right? She's so cute! Pixie's been keeping us busy, so I haven't had a chance to check out that hiking spot yet.",
        ),
        (
            "locomo:conv-44:session_2",
            4,
            "Andrew: Yeah, I get it. A puppy really takes some work. Has she settled in okay? Is she getting along with the other pups?",
        ),
        (
            "locomo:conv-44:session_2",
            5,
            "Audrey: Pixie's fitting in great! It took her a few days to get used to the other dogs, but now they're awesome friends. They love playing and exploring the house - so cute!",
        ),
    ] {
        ingest_turn(
            &tm.service,
            &format!("{group}:{position}"),
            group,
            position,
            content,
        )
        .await;
    }

    let items = AssembleContextCapability::assemble_context_from_service(
        &tm.service,
        query_text(
            "Which year did Audrey adopt the first three of her dogs?",
            None,
        ),
    )
    .await
    .expect("the original dog adoption context should assemble");

    assert!(
        items.iter().any(|item| item.content.contains("3 years")),
        "the original dogs' duration should remain available: {items:#?}"
    );
    assert!(
        items
            .iter()
            .any(|item| item.content.contains("collars and tags for my dogs")),
        "the original dog set should stay grounded: {items:#?}"
    );
    assert!(
        items.iter().all(|item| !item.content.contains("Pixie")),
        "a later puppy and its follow-up discussion do not answer when Audrey adopted the first three dogs: {items:#?}"
    );
}
