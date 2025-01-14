extern crate alloc;
use core::ffi::c_void;

use snafu::Snafu;

use crate::error::{bail_on, map_errno};

/// Creates a task to be run 'asynchronously' (More information at the [FreeRTOS docs](https://www.freertos.org/taskandcr.html)).
/// Takes in a closure that can move variables if needed.
/// If your task has a loop it is advised to use [`sleep(duration)`](sleep) so that the task does not take up necessary system resources.
/// Tasks should be long-living; starting many tasks can be slow and is usually not necessary.
pub fn spawn<F>(f: F) -> TaskHandle
where
    F: FnOnce() + Send + 'static,
{
    Builder::new().spawn(f).expect("Failed to spawn task")
}

fn spawn_inner<F: FnOnce() + Send + 'static>(
    function: F,
    priority: TaskPriority,
    stack_depth: TaskStackDepth,
    name: Option<&str>,
) -> Result<TaskHandle, SpawnError> {
    let mut entrypoint = TaskEntrypoint { function };
    let name = alloc::ffi::CString::new(name.unwrap_or("<unnamed>"))
        .unwrap()
        .into_raw();
    unsafe {
        let task = bail_on!(
            core::ptr::null(),
            pros_sys::task_create(
                Some(TaskEntrypoint::<F>::cast_and_call_external),
                &mut entrypoint as *mut _ as *mut c_void,
                priority as _,
                stack_depth as _,
                name,
            )
        );

        _ = alloc::ffi::CString::from_raw(name);
        Ok(TaskHandle { task })
    }
}

/// An owned permission to perform actions on a task.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskHandle {
    task: pros_sys::task_t,
}
unsafe impl Send for TaskHandle {}

impl TaskHandle {
    /// Pause execution of the task.
    /// This can have unintended consequences if you are not careful,
    /// for example, if this task is holding a mutex when paused, there is no way to retrieve it until the task is unpaused.
    pub fn pause(&self) {
        unsafe {
            pros_sys::task_suspend(self.task);
        }
    }

    /// Resumes execution of the task.
    pub fn unpause(&self) {
        unsafe {
            pros_sys::task_resume(self.task);
        }
    }

    /// Sets the task's priority, allowing you to control how much cpu time is allocated to it.
    pub fn set_priority(&self, priority: impl Into<u32>) {
        unsafe {
            pros_sys::task_set_priority(self.task, priority.into());
        }
    }

    /// Get the state of the task.
    pub fn state(&self) -> TaskState {
        unsafe { pros_sys::task_get_state(self.task).into() }
    }

    /// Send a notification to the task.
    pub fn notify(&self) {
        unsafe {
            pros_sys::task_notify(self.task);
        }
    }

    /// Waits for the task to finish, and then deletes it.
    pub fn join(self) {
        unsafe {
            pros_sys::task_join(self.task);
        }
    }

    /// Aborts the task and consumes it. Memory allocated by the task will not be freed.
    pub fn abort(self) {
        unsafe {
            pros_sys::task_delete(self.task);
        }
    }
}

/// An ergonomic builder for tasks. Alternatively you can use [`spawn`].
#[derive(Default)]
pub struct Builder<'a> {
    name: Option<&'a str>,
    priority: Option<TaskPriority>,
    stack_depth: Option<TaskStackDepth>,
}

impl<'a> Builder<'a> {
    /// Creates a task builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the name of the task, this is useful for debugging.
    pub fn name(mut self, name: &'a str) -> Self {
        self.name = Some(name);
        self
    }

    /// Sets the priority of the task (how much time the scheduler gives to it.).
    pub fn priority(mut self, priority: TaskPriority) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Sets how large the stack for the task is.
    /// This can usually be set to default
    pub fn stack_depth(mut self, stack_depth: TaskStackDepth) -> Self {
        self.stack_depth = Some(stack_depth);
        self
    }

    /// Builds and spawns the task
    pub fn spawn<F>(self, function: F) -> Result<TaskHandle, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        spawn_inner(
            function,
            self.priority.unwrap_or_default(),
            self.stack_depth.unwrap_or_default(),
            self.name,
        )
    }
}

/// Represents the current state of a task.
pub enum TaskState {
    /// The task is currently utilizing the processor
    Running,
    /// The task is currently yielding but may run in the future
    Ready,
    /// The task is blocked. For example, it may be [`sleep`]ing or waiting on a mutex.
    /// Tasks that are in this state will usually return to the task queue after a set timeout.
    Blocked,
    /// The task is suspended. For example, it may be waiting on a mutex or semaphore.
    Suspended,
    /// The task has been deleted using [`TaskHandle::abort`].
    Deleted,
    /// The task's state is invalid somehow
    Invalid,
}

impl From<u32> for TaskState {
    fn from(value: u32) -> Self {
        match value {
            pros_sys::E_TASK_STATE_RUNNING => Self::Running,
            pros_sys::E_TASK_STATE_READY => Self::Ready,
            pros_sys::E_TASK_STATE_BLOCKED => Self::Blocked,
            pros_sys::E_TASK_STATE_SUSPENDED => Self::Suspended,
            pros_sys::E_TASK_STATE_DELETED => Self::Deleted,
            pros_sys::E_TASK_STATE_INVALID => Self::Invalid,
            _ => Self::Invalid,
        }
    }
}

/// Represents how much time the cpu should spend on this task.
/// (Otherwise known as the priority)
#[repr(u32)]
pub enum TaskPriority {
    High = 16,
    Default = 8,
    Low = 1,
}

impl Default for TaskPriority {
    fn default() -> Self {
        Self::Default
    }
}

impl From<TaskPriority> for u32 {
    fn from(val: TaskPriority) -> Self {
        val as u32
    }
}

/// Represents how large of a stack the task should get.
/// Tasks that don't have any or many variables and/or don't need floats can use the low stack depth option.
#[repr(u32)]
pub enum TaskStackDepth {
    Default = 8192,
    Low = 512,
}

impl Default for TaskStackDepth {
    fn default() -> Self {
        Self::Default
    }
}

struct TaskEntrypoint<F> {
    function: F,
}

impl<F> TaskEntrypoint<F>
where
    F: FnOnce(),
{
    unsafe extern "C" fn cast_and_call_external(this: *mut c_void) {
        let this = this.cast::<Self>().read();

        (this.function)()
    }
}

#[derive(Debug, Snafu)]
pub enum SpawnError {
    #[snafu(display("The stack cannot be used as the TCB was not created."))]
    TCBNotCreated,
}

map_errno! {
    SpawnError {
        ENOMEM => SpawnError::TCBNotCreated,
    }
}

/// Sleeps the current task for the given amount of time.
/// This is especially useful in loops to provide a chance for other tasks to run.
pub fn sleep(duration: core::time::Duration) {
    unsafe { pros_sys::delay(duration.as_millis() as u32) }
}

/// Returns the task the function was called from.
pub fn current() -> TaskHandle {
    unsafe {
        TaskHandle {
            task: pros_sys::task_get_current(),
        }
    }
}

/// Gets the first notification in the queue.
/// If there is none, blocks until a notification is received.
/// I am unsure what happens if the thread is unblocked while waiting.
/// returns the value of the notification
pub fn get_notification() -> u32 {
    unsafe { pros_sys::task_notify_take(false, pros_sys::TIMEOUT_MAX) }
}
