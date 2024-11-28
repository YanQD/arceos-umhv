extern crate alloc;

use std::os::arceos::modules::axhal;
use std::os::arceos::modules::axtask;
use std::os::arceos::modules::axtask::TaskExtRef;


use alloc::boxed::Box;
use axtask::AxTaskRef;
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use timer_list::{TimeValue, TimerEvent, TimerList};

use crate::vmm::ipi::{IpiMessage, ipi_send_msg_by_core_id};

const TICKS_PER_SEC: u64 = 100;
const NANOS_PER_SEC: u64 = 1_000_000_000;
const PERIODIC_INTERVAL_NANOS: u64 = NANOS_PER_SEC / TICKS_PER_SEC;

pub struct VmmTimerEvent {
    task: AxTaskRef,
    timer_callback: Box<dyn FnOnce(TimeValue) + Send + 'static>,
}

impl VmmTimerEvent {
    /// Constructs a new [`VmmTimerEvent`] from a closure.
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce(TimeValue) + Send + 'static,
    {
        Self {
            task: axtask::current().as_task_ref().clone(),
            timer_callback: Box::new(f),
        }
    }
}

impl TimerEvent for VmmTimerEvent {
    fn callback(self, now: TimeValue) {
        let vcpu = self.task.task_ext().vcpu.clone();
        let to = vcpu.get_cpu_id();
        match to {
            Some(to) => {
                // TODO:给 to 发送附带参数的 IPI
                ipi_send_msg_by_core_id(to, IpiMessage::Timer(self));
            }
            _ => {
                (self.timer_callback)(now)
            }
        }
    }
}

#[percpu::def_percpu]
static TIMER_LIST: LazyInit<SpinNoIrq<TimerList<VmmTimerEvent>>> = LazyInit::new();

// deadline: ns
pub fn register_timer(deadline: u64, handler: VmmTimerEvent) {
    let timer_list = unsafe { TIMER_LIST.current_ref_mut_raw() };
    let mut timers = timer_list.lock();
    timers.set(TimeValue::from_nanos(deadline as u64), handler);
}

pub fn cancel_timer<F>(condition: F)
where
    F: Fn(&VmmTimerEvent) -> bool,
{
    let timer_list = unsafe { TIMER_LIST.current_ref_mut_raw() };
    let mut timers = timer_list.lock();
    timers.cancel(condition);
}

pub fn check_events() {
    // error!("check ev");
    loop {
        let now = axhal::time::wall_time();
        let timer_list = unsafe { TIMER_LIST.current_ref_mut_raw() };
        let event = timer_list.lock().expire_one(now);
        if let Some((_deadline, event)) = event {
            error!("pick one {:#?} to handler!!!", _deadline);
            event.callback(now);
        } else {
            break;
        }
    }
}

pub fn scheduler_next_event() {
    let now_ns = axhal::time::monotonic_time_nanos();
    let deadline = now_ns + PERIODIC_INTERVAL_NANOS;
    // error!("PHY deadline {} !!!", deadline);
    axhal::time::set_oneshot_timer(deadline);
}

pub fn init() {
    info!("Initing HV Timer...");
    // let timer_list = unsafe { TIMER_LIST.current_ref_mut_raw() };
    // timer_list.init_once(SpinNoIrq::new(TimerList::new()));

    // let res = axhal::irq::register_handler(axhal::time::TIMER_IRQ_NUM, || {
    //     info!("hv handler");
    //     check_events();
    //     scheduler_next_event();
    //     axtask::on_timer_tick();
    //     info!("hv handler end");
    // });
    
    // assert!(res == true);

    use std::os::arceos;
    use arceos::api::config;
    use arceos::api::task::{ax_set_current_affinity, AxCpuMask};
    use arceos::modules::axhal::cpu::this_cpu_id;

    use std::thread;

    use core::sync::atomic::AtomicUsize;
    use core::sync::atomic::Ordering;

    static CORES: AtomicUsize = AtomicUsize::new(0);

    for cpu_id in 0..config::SMP {
        // info!("spawning CPU{} init task ...", cpu_id);
        thread::spawn(move || {
            // Initialize cpu affinity here.
            assert!(
                ax_set_current_affinity(AxCpuMask::one_shot(cpu_id)).is_ok(),
                "Initialize CPU affinity failed!"
            );

            info!("Init HV timer in CPU{}", cpu_id);

            let timer_list = unsafe { TIMER_LIST.current_ref_mut_raw() };
            timer_list.init_once(SpinNoIrq::new(TimerList::new()));

            let _ = CORES.fetch_add(1, Ordering::Release);

            thread::yield_now();
        });
    }

    thread::yield_now();

    // info!("Go waiting!");

    // Wait for all cores
    while CORES.load(Ordering::Acquire) != config::SMP {
        core::hint::spin_loop();
    }
}
