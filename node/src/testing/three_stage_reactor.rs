use std::{
    fmt::{self, Display, Formatter},
    mem,
};

use derive_more::From;
use serde::Serialize;
use tokio::runtime::Handle;
use tracing::warn;
use utils::WithDir;

use crate::{
    effect::{EffectBuilder, Effects},
    reactor::{
        initializer::Reactor as InitializerReactor, joiner::Reactor as JoinerReactor,
        validator::Reactor as ValidatorReactor, wrap_effects, EventQueueHandle, QueueKind, Reactor,
        Scheduler,
    },
    utils, NodeRng,
};

#[derive(Copy, Clone, Debug)]
enum Stage {
    NotStarted,
    Initializing,
    Joining,
    Validating,
}

enum ThreeStageReactor {
    NotStarted,
    Initializer(
        InitializerReactor,
        EventQueueHandle<<InitializerReactor as Reactor>::Event>,
    ),
    Joiner(
        JoinerReactor,
        EventQueueHandle<<JoinerReactor as Reactor>::Event>,
    ),
    Validator(
        ValidatorReactor,
        EventQueueHandle<<ValidatorReactor as Reactor>::Event>,
    ),
}

impl ThreeStageReactor {
    fn stage(&self) -> Stage {
        match self {
            ThreeStageReactor::NotStarted => Stage::NotStarted,
            ThreeStageReactor::Initializer(_, _) => Stage::Initializing,
            ThreeStageReactor::Joiner(_, _) => Stage::Joining,
            ThreeStageReactor::Validator(_, _) => Stage::Validating,
        }
    }
}

#[derive(Debug, From, Serialize)]
enum ThreeStageEvent {
    #[from]
    InitializerEvent(<InitializerReactor as Reactor>::Event),
    #[from]
    JoinerEvent(<JoinerReactor as Reactor>::Event),
    #[from]
    ValidatorEvent(<ValidatorReactor as Reactor>::Event),
}

#[derive(Debug)]
enum ThreeStageError {
    InitializerError(<InitializerReactor as Reactor>::Error),
    JoinerError(<JoinerReactor as Reactor>::Error),
    ValidatorError(<ValidatorReactor as Reactor>::Error),
}

impl Display for ThreeStageEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ThreeStageEvent::InitializerEvent(ev) => {
                write!(f, "init_ev: {}", ev)
            }
            ThreeStageEvent::JoinerEvent(ev) => {
                write!(f, "join_ev: {}", ev)
            }
            ThreeStageEvent::ValidatorEvent(ev) => {
                write!(f, "vald_ev: {}", ev)
            }
        }
    }
}

impl Reactor for ThreeStageReactor {
    type Event = ThreeStageEvent;

    type Config = <InitializerReactor as Reactor>::Config;

    type Error = ThreeStageError;

    fn dispatch_event(
        &mut self,
        effect_builder: EffectBuilder<Self::Event>,
        rng: &mut NodeRng,
        event: ThreeStageEvent,
    ) -> Effects<Self::Event> {
        let stage = self.stage();
        let mut should_transition = false;

        let mut tsr = ThreeStageReactor::NotStarted;
        mem::swap(&mut tsr, self);

        let effects = match (event, &mut tsr) {
            (
                ThreeStageEvent::InitializerEvent(ev),
                ThreeStageReactor::Initializer(ref mut reactor, event_queue_handle),
            ) => {
                let effect_builder = EffectBuilder::new(*event_queue_handle);

                let effects = wrap_effects(
                    ThreeStageEvent::InitializerEvent,
                    reactor.dispatch_event(effect_builder, rng, ev),
                );

                if reactor.is_stopped() {
                    if !reactor.stopped_successfully() {
                        panic!("failed to transition from initializer to joiner");
                    }

                    should_transition = true;
                }

                effects
            }
            (
                ThreeStageEvent::JoinerEvent(ev),
                ThreeStageReactor::Joiner(ref mut reactor, event_queue_handle),
            ) => {
                let effect_builder = EffectBuilder::new(*event_queue_handle);

                wrap_effects(
                    ThreeStageEvent::JoinerEvent,
                    reactor.dispatch_event(effect_builder, rng, ev),
                )
            }
            (
                ThreeStageEvent::ValidatorEvent(ev),
                ThreeStageReactor::Validator(ref mut reactor, event_queue_handle),
            ) => {
                let effect_builder = EffectBuilder::new(*event_queue_handle);

                wrap_effects(
                    ThreeStageEvent::ValidatorEvent,
                    reactor.dispatch_event(effect_builder, rng, ev),
                )
            }
            (event, former_self) => {
                let stage = former_self.stage();

                warn!(
                    ?event,
                    ?stage,
                    "discarded event due to not being in the right stage"
                );

                Effects::new()
            }
        };

        if should_transition {
            match tsr {
                ThreeStageReactor::NotStarted => {
                    // We will never run a `NotStarted` stage.
                    unreachable!()
                }
                ThreeStageReactor::Initializer(initializer_reactor, initializer_queue) => {
                    assert!(initializer_queue.is_empty());

                    let joiner_scheduler = utils::leak(Scheduler::new(QueueKind::weights()));
                    let joiner_queue = EventQueueHandle::new(joiner_scheduler);

                    tokio::spawn(forward_to_queue(
                        joiner_scheduler,
                        effect_builder.into_inner(),
                    ));

                    let (joiner_reactor, joiner_effects) = JoinerReactor::new(
                        WithDir::new("TODO", initializer_reactor),
                        todo!(),
                        joiner_queue,
                        rng,
                    )
                    .expect("joiner initialization failed");

                    *self = ThreeStageReactor::Joiner(joiner_reactor, joiner_queue);

                    effects.extend(
                        wrap_effects(ThreeStageEvent::JoinerEvent, joiner_effects).into_iter(),
                    )
                }
                ThreeStageReactor::Joiner(joiner_reactor, joiner_queue) => {
                    // TODO: We might not be able to assert this, as there may be data coming in
                    // that has not been handled. This will lead to dropped responders!
                    assert!(joiner_queue.is_empty());

                    // `into_validator_config` is just waiting for networking sockets to shut down
                    // and will not stall on disabled event processing, so it is
                    // safe to block here.
                    let rt = Handle::current();
                    let validator_config = rt.block_on(joiner_reactor.into_validator_config());

                    // This might be wrong, remove this check.
                    assert!(effects.is_empty(),
                    "before transitioning from joiner to validator, the returned effects should be empty");

                    let validator_scheduler = utils::leak(Scheduler::new(QueueKind::weights()));
                    let validator_queue = EventQueueHandle::new(validator_scheduler);

                    tokio::spawn(forward_to_queue(
                        validator_scheduler,
                        effect_builder.into_inner(),
                    ));

                    let (validator_reactor, validator_effects) =
                        ValidatorReactor::new(validator_config, todo!(), validator_queue, rng)
                            .expect("validator intialization failed");

                    *self = ThreeStageReactor::Validator(validator_reactor, validator_queue);

                    effects.extend(
                        wrap_effects(ThreeStageEvent::ValidatorEvent, validator_effects)
                            .into_iter(),
                    )
                }
                ThreeStageReactor::Validator(_, _) => {
                    // We're not transitioning from a validator reactor.
                    unreachable!()
                }
            }
        }

        effects
    }

    fn new(
        cfg: Self::Config,
        registry: &prometheus::Registry,
        event_queue: EventQueueHandle<Self::Event>,
        rng: &mut NodeRng,
    ) -> Result<(Self, Effects<Self::Event>), Self::Error> {
        let initializer_scheduler = utils::leak(Scheduler::new(QueueKind::weights()));
        let initializer_queue: EventQueueHandle<<InitializerReactor as Reactor>::Event> =
            EventQueueHandle::new(initializer_scheduler);

        tokio::spawn(forward_to_queue(initializer_scheduler, event_queue));

        let (initializer, initializer_effects) =
            InitializerReactor::new(cfg, registry, initializer_queue, rng)
                .map_err(ThreeStageError::InitializerError)?;

        Ok((
            ThreeStageReactor::Initializer(initializer, initializer_queue),
            wrap_effects(ThreeStageEvent::InitializerEvent, initializer_effects),
        ))
    }
}

/// Long-running task that forwards events arriving on one scheduler to another.
async fn forward_to_queue<I, O>(source: &Scheduler<I>, target_queue: EventQueueHandle<O>)
where
    O: From<I>,
{
    // Note: This will keep waiting forever if the sending end disappears, which is fine for tests.
    loop {
        let (event, queue_kind) = source.pop().await;
        target_queue.schedule(event, queue_kind);
    }
}
