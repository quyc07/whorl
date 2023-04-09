//! # A Whorlwind Tour in Building a Rust Async Executor
//!
//! whorl is a self contained library to run asynchronous Rust code with the
//! following goals in mind:
//!
//! - Keep it in one file. You should be able to read this code beginning to end
//!   like a literate program and understand what each part does and how it fits
//!   into the larger narrative. The code is organized to tell a story, not
//!   necessarily how I would normally structure Rust code.
//! - Teach others what is going on when you run async code in Rust with a runtime
//!   like tokio. There is no magic, just many synchronous functions in an async
//!   trenchcoat.
//! - Explain why different runtimes are incompatible, even if they all run async
//!   programs.
//! - Only use the `std` crate to show that yes all the tools to build one exist
//!   and if you wanted to, you could.
//! - Use only stable Rust. You can build this today; no fancy features needed.
//! - Explain why `std` doesn't ship an executor, but just the building blocks.
//!
//! What whorl isn't:
//! - Performant, this is an adaptation of a class I gave at Rustconf a few
//!   years back. Its first and foremost goal is to teach *how* an executor
//!   works, not the best way to make it fast. Reading the tokio source
//!   code would be a really good thing if you want to learn about how to make
//!   things performant and scalable.
//! - "The Best Way". Programmers have opinions, I think we should maybe have
//!   less of them sometimes. Even me. You might disagree with an API design
//!   choice or a way I did something here and that's fine. I just want you to
//!   learn how it all works.
//! - An introduction to Rust. This assumes you're somewhat familiar with it and
//!   while I've done my best to break it down so that it is easy to understand,
//!   that just might not be the case and I might gloss over details given I've
//!   done Rust for over 6 years at this point. Expert blinders are real and if
//!   things are confusing, do let me know in the issue tracker. I'll try my best
//!   to make it easier to grok, but if you've never touched Rust before, this is
//!   in all honesty not the best place to start.
//!
//! With all of that in mind, let's dig into it all!

use crate::runtime::Spawner;
use chrono::Local;
use std::fmt::Debug;
use std::thread;

pub mod futures {
    //! This is our module to provide certain kinds of futures to users. In the case
    //! of our [`Sleep`] future here, this is not dependent on the runtime in
    //! particular. We would be able to run this on any executor that knows how to
    //! run a future. Where incompatibilities arise is if you use futures or types
    //! that depend on the runtime or traits not defined inside of the standard
    //! library. For instance, `std` does not provide an `AsyncRead`/`AsyncWrite`
    //! trait as of Oct 2021. As a result, if you want to provide the functionality
    //! to asynchronously read or write to something, then that trait tends to be
    //! written for an executor. So tokio would have its own `AsyncRead` and so
    //! would ours for instance. Now if a new library wanted to write a type that
    //! can, say, read from a network socket asynchronously, they'd have to write an
    //! implementation of `AsyncRead` for both executors. Not great. Another way
    //! incompatibilities can arise is when those futures depend on the state of the
    //! runtime itself. Now that implementation is locked to the runtime.
    //!
    //! Sometimes this is actually okay; maybe the only way to implement
    //! something is depending on the runtime state. In other ways it's not
    //! great. Things like `AsyncRead`/`AsyncWrite` would be perfect additions
    //! to the standard library at some point since they describe things that
    //! everyone would need, much like how `Read`/`Write` are in stdlib and we
    //! all can write generic code that says I will work with anything that I
    //! can read or write to.
    //!
    //! This is why, however, things like Future, Context, Wake, Waker etc. all
    //! the components we need to build an executor are in the standard library.
    //! It means anyone can build an executor and accept most futures or work
    //! with most libraries without needing to worry about which executor they
    //! use. It reduces the burden on maintainers and users. In some cases
    //! though, we can't avoid it. Something to keep in mind as you navigate the
    //! async ecosystem and see that some libraries can work on any executor or
    //! some ask you to opt into which executor you want with a feature flag.
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
        time::SystemTime,
    };

    /// A future that will allow us to sleep and block further execution of the
    /// future it's used in without blocking the thread itself. It will be
    /// polled and if the timer is not up, then it will yield execution to the
    /// executor.
    /// 用以阻塞当前线程的future，不会阻塞线程，而是阻塞当前future的执行，直到时间到了，才会继续执行当前future，
    /// 而不是阻塞线程，所以不会阻塞其他future的执行，这是一个异步的sleep，而不是同步的sleep，同步的sleep会阻塞线程，
    /// 也就是说，同步的sleep会阻塞其他future的执行。
    pub struct Sleep {
        /// What time the future was created at, not when it was started to be
        /// polled.
        /// future创建的时间，不是开始poll的时间
        now: SystemTime,
        /// How long in the future in ms we must wait till we return
        /// that the future has finished polling.
        /// future需要等待的时间，单位是ms，也就是说，如果这个值是1000，那么就是1s，如果是2000，那么就是2s，以此类推
        ms: u128,
    }

    impl Sleep {
        /// A simple API whereby we take in how long the consumer of the API
        /// wants to sleep in ms and set now to the time of creation and
        /// return the type itself, which is a Future.
        /// 一个简单的API，接收一个参数，这个参数是需要等待的时间，单位是ms，并设置当前时间为Sleep创建时间，
        /// 返回一个future，这个future就是Sleep。
        pub fn new(ms: u128) -> Self {
            Self {
                now: SystemTime::now(),
                ms,
            }
        }
    }

    impl Future for Sleep {
        /// We don't need to return a value for [`Sleep`], as we just want it to
        /// block execution for a while when someone calls `await` on it.
        /// [`Sleep`]不需要返回值，因为我们只是想让它阻塞一段时间，直到时间到了，才会继续执行当前future
        /// 这里的Output就是返回值的类型，因为我们不需要返回值，所以这里就是()，也就是空元组。
        type Output = ();
        /// The actual implementation of the future, where you can call poll on
        /// [`Sleep`] if it's pinned and the pin has a mutable reference to
        /// [`Sleep`]. In this case we don't need to utilize
        /// [`Context`][std::task::Context] here and in fact you often will not.
        /// It only serves to provide access to a `Waker` in case you need to
        /// wake the task. Since we always do that in our executor, we don't need
        /// to do so here, but you might find if you manually write a future
        /// that you need access to the waker to wake up the task in a special
        /// way. Waking up the task just means we put it back into the executor
        /// to be polled again.
        /// 实现future的poll方法，这个方法会被调用，如果future被pin了，并且pin有一个可变的引用指向Sleep，
        /// 在这个方法里，我们不需要使用Context，因为我们的executor会自动调用这个方法，所以我们不需要手动调用，
        /// 但是如果你手动实现一个future，你可能需要使用Context，因为你需要手动调用这个方法，而不是让executor自动调用。
        /// 这个方法的返回值是Poll<Self::Output>，也就是Poll<()>。
        /// Poll是一个枚举，有两个值，Pending和Ready，Pending表示future还没有准备好，需要再次调用poll方法，
        /// Ready表示future已经准备好了，可以继续执行。
        fn poll(self: Pin<&mut Self>, _: &mut Context) -> Poll<Self::Output> {
            // If enough time has passed, then when we're polled we say that
            // we're ready and the future has slept enough. If not, we just say
            // that we're pending and need to be re-polled, because not enough
            // time has passed.
            if self.now.elapsed().unwrap().as_millis() >= self.ms {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    // In practice, what we do when we sleep is something like this:
    // ```
    // async fn example() {
    //     Sleep::new(2000).await;
    // }
    // ```
    //
    // Which is neat and all but how is that future being polled? Well, this
    // all desugars out to:
    // ```
    // fn example() -> impl Future<Output = ()> {
    //     let mut sleep = Sleep::new(2000);
    //     loop {
    //        match Pin::new(sleep).as_mut().poll(&mut context) {
    //            Poll::Ready(()) => (),
    //            // You can't
    //            Poll::Pending => yield,
    //        }
    //     }
    // }
}

#[test]
/// To understand what we'll build, we need to see and understand what we will
/// run and the output we expect to see. Note that if you wish to run this test,
/// you should use the command `cargo test -- --nocapture` so that you can see
/// the output of `println` being used, otherwise it'll look like nothing is
/// happening at all for a while.
/// 为了理解我们要构建的内容，我们需要看一下我们要运行的内容，以及我们期望看到的输出。
/// 注意，如果你想要运行这个测试，你应该使用`cargo test -- --nocapture`命令，这样你就可以看到`println`的输出，
/// 否则，你会看到一段时间什么都没有发生。
fn library_test() {
    // We're going to import our Sleep future to make sure that it works,
    // because it's not a complicated future and it's easy to see the
    // asynchronous nature of the code.
    use crate::{futures::Sleep, runtime};
    // We want some random numbers so that the sleep futures finish at different
    // times. If we didn't, then the code would look synchronous in nature even
    // if it isn't. This is because we schedule and poll tasks in what is
    // essentially a loop unless we use block_on.
    use rand::Rng;
    // We need to know the time to show when a future completes. Time is cursed
    // and it's best we dabble not too much in it.
    use std::time::SystemTime;

    println!(
        "1. Current thread name {} {} {}",
        thread::current().name().unwrap(),
        current_thread_id(),
        current_time()
    );
    // This function causes the runtime to block on this future. It does so by
    // just taking this future and polling it till completion in a loop and
    // ignoring other tasks on the queue. Sometimes you need to block on async
    // functions and treat them as sync. A good example is running a webserver.
    // You'd want it to always be running, not just sometimes, and so blocking
    // it makes sense. In a single threaded executor this would block all
    // execution. In our case our executor is single-threaded. Technically it
    // runs on a separate thread from our program and so blocks running other
    // tasks, but the main function will keep running. This is why we call
    // `wait` to make sure we wait till all futures finish executing before
    // exiting.
    // block_on方法会创建一个新的Runtime，然后调用这个方法。Runtime通过获取当前future而且不同的轮训执行poll方法，
    // 直到future返回Ready，并且忽略其他的任务。有时候我们需要阻塞异步函数，把它当做同步函数来使用。
    // 一个很好的例子就是运行一个web服务器，你希望它一直运行，而不是偶尔运行，所以阻塞它是有意义的。
    // 在一个单线程的executor里，这会阻塞所有的执行。在我们的例子里，executor是单线程的。
    // 从技术上讲，它在一个单独的线程上运行，所以它会阻塞运行其他的任务，但是main函数会一直运行。
    // 这就是为什么我们调用wait方法，来确保我们等待所有的future执行完毕，然后再退出。
    runtime::block_on(async {
        const SECOND: u128 = 1000; //ms
        println!(
            "2. Begin Asynchronous Execution,{}, {}",
            current_thread_id(),
            current_time()
        );
        // Create a random number generator so we can generate random numbers
        // 创建一个随机数生成器，这样我们就可以生成随机数
        let mut rng = rand::thread_rng();

        // Spawn 5 different futures on our executor
        // 生成5个不同的future，然后在executor上执行
        for i in 0..5 {
            // Generate the two numbers between 1 and 9. We'll spawn two futures
            // that will sleep for as many seconds as the random number creates
            // 生成两个1到9之间的随机数。我们会生成两个future，这两个future会睡眠多少秒，取决于随机数的大小
            let random = rng.gen_range(1..5);
            let random2 = rng.gen_range(1..5);

            // We now spawn a future onto the runtime from within our future
            // 我们现在在future里面，从runtime上生成一个future
            runtime::spawn(async move {
                println!(
                    "Spawned Fn #{:02}: Start {} {}",
                    i,
                    current_thread_id(),
                    current_time()
                );
                // This future will sleep for a certain amount of time before
                // continuing execution
                // 这个future会睡眠一段时间，然后继续执行
                Sleep::new(SECOND * random).await;
                // After the future waits for a while, it then spawns another
                // future before printing that it finished. This spawned future
                // then sleeps for a while and then prints out when it's done.
                // Since we're spawning futures inside futures, the order of
                // execution can change.
                // 在future等待一段时间后，它会生成另一个future，然后打印它已经完成。
                // 这个生成的future会睡眠一段时间，然后打印它已经完成。
                // 由于我们在future里面生成future，所以执行的顺序会改变。
                runtime::spawn(async move {
                    Sleep::new(SECOND * random2).await;
                    println!(
                        "Spawned Fn #{:02}: Inner {} {}",
                        i,
                        current_thread_id(),
                        current_time()
                    );
                });
                println!(
                    "Spawned Fn #{:02}: Ended {} {}",
                    i,
                    current_thread_id(),
                    current_time()
                );
            });
        }
        // To demonstrate that block_on works we block inside this future before
        // we even begin polling the other futures.
        // 为了演示block_on的工作原理，我们在future里面阻塞，然后再开始轮训其他的future。
        runtime::block_on(async {
            // This sleeps longer than any of the spawned functions, but we poll
            // this to completion first even if we await here.
            // 这个睡眠的时间比生成的future都长，但是我们会先轮训这个future，即使我们在这里await。
            Sleep::new(3000).await;
            println!(
                "3. Blocking Function Polled To Completion {} {}",
                current_thread_id(),
                current_time()
            );
        });
    });

    // We now wait on the runtime to complete each of the tasks that were
    // spawned before we exit the program
    // 现在我们等待runtime完成所有的任务，然后再退出程序
    runtime::wait();
    println!(
        "4. End of Asynchronous Execution {} {}",
        current_thread_id(),
        current_time()
    );

    // When all is said and done when we run this test we should get output that
    // looks somewhat like this (though in different order):
    //
    // Begin Asynchronous Execution
    // Blocking Function Polled To Completion
    // Spawned Fn #00: Start 1634664688
    // Spawned Fn #01: Start 1634664688
    // Spawned Fn #02: Start 1634664688
    // Spawned Fn #03: Start 1634664688
    // Spawned Fn #04: Start 1634664688
    // Spawned Fn #01: Ended 1634664690
    // Spawned Fn #01: Inner 1634664691
    // Spawned Fn #04: Ended 1634664694
    // Spawned Fn #04: Inner 1634664695
    // Spawned Fn #00: Ended 1634664697
    // Spawned Fn #02: Ended 1634664697
    // Spawned Fn #03: Ended 1634664697
    // Spawned Fn #00: Inner 1634664698
    // Spawned Fn #03: Inner 1634664698
    // Spawned Fn #02: Inner 1634664702
    // End of Asynchronous Execution
}

// 获取当前线程唯一id
pub fn current_thread_id() -> String {
    let thread = thread::current();
    let id = thread.id();
    format!("{:?}", id)
}

// 获取当前时间 yyyy-MM-dd HH:MM:ss
fn current_time() -> String {
    let now = Local::now();
    now.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub mod lazy {
    use std::{
        // We don't want to use `static mut` since that's UB and so instead we need
        // a way to set our statics for our code at runtime. Since we want this to
        // work across threads, we can't use `Cell` or `RefCell` here, and since it's
        // a static we can't use a `Mutex` as its `new` function is not const. That
        // means we need to use the actual type that all of these types use to hold
        // the data: [`UnsafeCell`]! We'll see below where this is used and how, but
        // just know that this will let us set some global values at runtime!
        // 我们不想使用`static mut`，因为这是UB，所以我们需要一种方法来在运行时设置我们的代码的静态变量。
        // 由于我们希望它能跨线程工作，所以我们不能在这里使用`Cell`或`RefCell`，而且由于它是一个静态变量，
        // 我们不能使用`Mutex`，因为它的`new`函数不是const。这意味着我们需要使用这些类型用来保存数据的实际类型：[`UnsafeCell`]！
        // 我们将在下面看到它的使用方式，但是要知道，这将让我们在运行时设置一些全局值！
        cell::UnsafeCell,
        mem::{
            // If you want to import the module to use while also specifying other
            // imports you can use self to do that. In this case it will let us call
            // `mem::swap` while also letting us just use `MaybeUninit` without any
            // extra paths prepended to it. I tend to do this for functions that are
            // exported at the module level and not encapsulated in a type so that
            // it's more clear where it comes from, but that's a personal
            // preference! You could just as easily import `swap` here instead!
            // 如果你想导入模块来使用，同时还指定其他导入，你可以使用self来做到这一点。
            // 在这种情况下，它将让我们调用`mem::swap`，同时让我们只使用`MaybeUninit`而不需要任何额外的路径前缀。
            // 我倾向于为模块级别导出的函数而不是封装在类型中的函数执行此操作，以便更清楚地了解它来自哪里，但这是个人偏好！
            // 你也可以在这里导入`swap`！
            self,
            // `MaybeUninit` is the only way to represent a value that's possibly
            // uninitialized without causing instant UB with `std::mem::uninitialized`
            // or `std::mem::zeroed`. There's more info in the docs here:
            // https://doc.rust-lang.org/stable/std/mem/union.MaybeUninit.html#initialization-invariant
            //
            // We need this so that we can have an UnsafeCell with nothing inside it
            // until we initialize it once and only once without causing UB and
            // having nasal demons come steal random data and give everyone a bad
            // time.
            // `MaybeUninit`是表示可能未初始化的值的唯一方法，而不会导致使用`std::mem::uninitialized`或`std::mem::zeroed`立即产生UB。
            // 有关更多信息，请参阅文档：https://doc.rust-lang.org/stable/std/mem/union.MaybeUninit.html#initialization-invariant
            // 我们需要使用它来保证一个没有任何数据的UnsafeCell不会导致UB，直到我们对其进行唯一一次初始化。
            MaybeUninit,
        },
        // Sometimes you need to make sure that something is done once and
        // only once. We also might want to make sure that no matter on what
        // thread this holds true. Enter `Once`, a really great synchronization
        // type that's around for just this purpose. It also has the nice
        // property that if, say, it gets called to be initialized across many
        // threads that it only runs the initialization function once and has
        // the other threads wait until it's done before letting them continue
        // with their execution.
        // 有时你需要确保某件事只做一次。我们还可能希望确保无论在哪个线程上都是如此。`Once`，一个非常好的同步类型，就是为了这个目的。
        // 它还具有一个很好的属性，即如果它被用在多线程同时初始化时，则它保证只运行初始化函数一次，并且其他线程等待直到它完成，然后才让它们继续执行。
        sync::Once,
    };

    /// We want to have a static value that's set at runtime and this executor will
    /// only use libstd. As of 10/26/21, the lazy types in std are still only on
    /// nightly and we can't use another crate, so crates like `once_cell` and
    /// `lazy_static` are also out. Thus, we create our own Lazy type so that it will
    /// calculate the value only once and only when we need it.
    /// 我们想要一个在运行时设置的静态值，而且这个执行器只使用标准库。
    /// 截至2021年10月26日，std中的延迟类型仍然只在夜间版中，我们不能使用另一个crate，因此像`once_cell`和`lazy_static`这样的crate也不行。
    /// 因此，我们创建自己的Lazy类型，以便它只计算一次值，并且仅在我们需要时才计算。
    pub struct Lazy<T> {
        /// `Once` is a neat synchronization primitive that we just talked about
        /// and this is where we need it! We want to make sure we only write into
        /// the value of the Lazy type once and only once. Otherwise we'd have some
        /// really bad things happen if we let static values be mutated. It'd break
        /// thread safety!
        /// Once是一个很好的同步原语，我们刚刚谈到了它，这就是我们需要它的地方！我们想确保我们只将值写入Lazy类型一次且仅一次。
        /// 否则，如果我们让静态值被改变，就会发生一些非常糟糕的事情。它会破坏线程安全！
        once: Once,
        /// The cell is where we hold our data. The use of `UnsafeCell` is what lets
        /// us sidestep Rust's guarantees, provided we actually use it correctly and
        /// still uphold those guarantees. Rust can't always validate that
        /// everything is safe, even if it is, and so the flexibility it provides
        /// with certain library types and unsafe code lets us handle those cases
        /// where the compiler cannot possibly understand it's okay. We also use the
        /// `MaybeUninit` type here to avoid undefined behavior with uninitialized
        /// data. We'll need to drop the inner value ourselves though to avoid
        /// memory leaks because data may not be initialized and so the type won't
        /// call drop when it's not needed anymore. We could get away with not doing
        /// it though since we're only using it for static values, but let's be
        /// thorough here!
        /// cell是我们保存数据的地方。使用`UnsafeCell`是让我们绕过Rust的保证限制，只要我们正确使用它并且仍然遵守这些保证。
        /// Rust不总是能够确认程序是安全的，即使它确实是安全的，因此UnSafe代码提供的灵活性使我们能够处理编译器无法理解的情况。
        /// 我们还在这里使用`MaybeUninit`类型来避免未初始化数据的未定义行为。但是我们需要自己丢弃内部值，以避免内存泄漏，因为数据可能未初始化，
        /// 因此`MaybeUninit`不会在不再需要时调用drop。虽然我们只使用它来保存静态值，但我们还是要彻底地做到这一点！
        cell: UnsafeCell<MaybeUninit<T>>,
    }

    impl<T> Lazy<T> {
        /// We must construct the type using a const fn so that it can be used in
        /// `static` contexts. The nice thing is that all of the function calls we
        /// make here are also const and so this will just work. The compiler will
        /// figure it all out and make sure the `Lazy` static value exists in our
        /// final binary.
        /// 我们必须使用const fn来构造类型，以便它可以在`static`上下文中使用。
        /// 好的是，我们在这里调用的所有函数都是const的，因此这将正常工作。
        /// 编译器会弄清楚一切，并确保`Lazy`静态值存在于我们的最终二进制文件中。
        pub const fn new() -> Self {
            Self {
                once: Once::new(),
                cell: UnsafeCell::new(MaybeUninit::uninit()),
            }
        }
        /// We want a way to check if we have initialized the value so that we can
        /// get the value from cell without causing who knows what kind of bad
        /// things if we read garbage data.
        /// 我们想要一种方法来检查我们是否已经初始化了值，以便我们可以从cell中获取值，而不会导致我们读取垃圾数据而引起未知的坏事情。
        fn is_initialized(&self) -> bool {
            self.once.is_completed()
        }

        /// This function will either grab a reference to the type or creates it
        /// with a given function
        ///
        pub fn get_or_init(&self, func: fn() -> T) -> &T {
            self.once.call_once(|| {
                // /!\ SAFETY /!\: We only ever write to the cell once
                //
                // We first get a `*mut MaybeUninit` to the cell and turn it into a
                // `&mut MaybeUninit`. That's when we call `write` on `MaybeUninit`
                // to pass the value of the function into the now initialized
                // `MaybeUninit`.
                // 这是安全的！我们只会将值写入一次。
                // 我们首先获取一个`*mut MaybeUninit`到cell，并将其转换为`&mut MaybeUninit`。
                // 这时我们调用`MaybeUninit`上的`write`将函数的值传递到现在初始化的`MaybeUninit`。
                (unsafe { &mut *self.cell.get() }).write(func());
            });
            // /!\ SAFETY /!\: We already made sure `Lazy` was initialized with our call to
            // `call_once` above
            //
            // We now want to actually retrieve the value we wrote so that we can
            // use it! We get the `*mut MaybeUninit` from the cell and turn it into
            // a `&MaybeUninit` which then lets us call `assume_init_ref` to get
            // the `&T`. This function - much like `get` - is also unsafe, but since we
            // know that the value is initialized it's fine to call this!
            // 这是安全的！我们已经确保`Lazy`已经初始化了。
            // 我们现在想要实际获取我们写入的值，以便我们可以使用它！
            // 我们从cell中获取`*mut MaybeUninit`并将其转换为`&MaybeUninit`，然后我们就可以调用`assume_init_ref`来获取`&T`。
            // `assume_init_ref`这个函数 - 就像`get`一样 - 也是不安全的，但是由于我们知道值已经初始化，所以可以调用这个函数！
            unsafe { &(*self.cell.get()).assume_init_ref() }
        }
    }

    /// We now need to implement `Drop` by hand specifically because `MaybeUninit`
    /// will need us to drop the value it holds by ourselves only if it exists. We
    /// check if the value exists, swap it out with an uninitialized value and then
    /// change `MaybeUninit<T>` into just a `T` with a call to `assume_init` and
    /// then call `drop` on `T` itself
    /// 我们现在需要手动实现`Drop`，特别是因为`MaybeUninit`需要我们自己丢弃它所持有的值，只有当它存在时才需要这样做。
    /// 我们检查值是否存在，将其与未初始化的值交换，然后将`MaybeUninit<T>`转换为`T`，并通过调用`assume_init`来改变`T`，然后调用`T`上的`drop`。
    impl<T> Drop for Lazy<T> {
        fn drop(&mut self) {
            if self.is_initialized() {
                let old = mem::replace(unsafe { &mut *self.cell.get() }, MaybeUninit::uninit());
                drop(unsafe { old.assume_init() });
            }
        }
    }

    /// Now you might be asking yourself why we are implementing these traits by
    /// hand and also why it's unsafe to do so. `UnsafeCell`is the big reason here
    /// and you can see this by commenting these two lines and trying to compile the
    /// code. Because of how auto traits work then if any part is not `Send` and
    /// `Sync` then we can't use `Lazy` for a static. Note that auto traits are a
    /// compiler specific thing where if everything in a type implements a trait
    /// then that type also implements it. `Send` and `Sync` are great examples of
    /// this where any type becomes `Send` and/or `Sync` if all its types implement
    /// them too! `UnsafeCell` specifically implements !Sync and since it is not
    /// `Sync` then it can't be used in a `static`. We can override this behavior
    /// though by implementing these traits for `Lazy` here though. We're saying
    /// that this is okay and that we uphold the invariants to be `Send + Sync`. We
    /// restrict it though and say that this is only the case if the type `T`
    /// *inside* `Lazy` is `Sync` only if `T` is `Send + Sync`. We know then that
    /// this is okay because the type in `UnsafeCell` can be safely referenced
    /// through an `&'static` and that the type it holds is also safe to use across
    /// threads. This means we can set `Lazy` as `Send + Sync` even though the
    /// internal `UnsafeCell` is !Sync in a safe way since we upheld the invariants
    /// for these traits.
    /// 现在你可能会问自己为什么我们要手动实现这些trait，以及为什么这样做是不安全的。
    /// `UnsafeCell`是这里的一个重要原因，你可以通过注释掉这两行并尝试编译代码来看到这一点。
    /// 由于自动trait的工作方式，如果任何部分不是`Send`和`Sync`，那么我们就不能在静态变量中使用`Lazy`。
    /// 请注意，自动trait是编译器特定的东西，如果类型中的所有部分都实现了trait，那么该类型也会实现该trait。
    /// `Send`和`Sync`就是很好的例子，如果类型中所有部分都实现了它们，那么任何类型也都会变成`Send`和/或`Sync`。
    /// `UnsafeCell`特别实现了!Sync，因为它不是`Sync`，所以它不能在`static`中使用。
    /// 我们可以通过在这里为`Lazy`实现这些trait来覆盖此行为。
    /// 也就是说，我们确认只有在`Lazy`内部的类型`T`是`Sync`时才是`Send + Sync`，以此保证了`Send + Sync`的不变量。
    /// 我们知道这是可以的，因为`UnsafeCell`中的类型可以通过`&'static`安全地引用，并且它所持有的类型也可以安全地跨线程使用。
    /// 这意味着我们可以在安全的方式中将`Lazy`设置为`Send + Sync`，即使内部的`UnsafeCell`是!Sync，因为我们保证了这些trait的不变量。
    unsafe impl<T: Send> Send for Lazy<T> {}

    unsafe impl<T: Send + Sync> Sync for Lazy<T> {}
}

pub mod runtime {
    use crate::{current_thread_id, current_time};
    use std::time::SystemTime;
    use std::{
        // We need a place to put the futures that get spawned onto the runtime
        // somewhere and while we could use something like a `Vec`, we chose a
        // `LinkedList` here. One reason being that we can put tasks at the front of
        // the queue if they're a blocking future. The other being that we use a
        // constant amount of memory. We only ever use as much as we need for tasks.
        // While this might not matter at a small scale, this does at a larger
        // scale. If your `Vec` never gets smaller and you have a huge burst of
        // tasks under, say, heavy HTTP loads in a web server, then you end up eating
        // up a lot of memory that could be used for other things running on the
        // same machine. In essence what you've created is a kind of memory leak
        // unless you make sure to resize the `Vec`. @mycoliza did a good Twitter
        // thread on this here if you want to learn more!
        //
        // https://twitter.com/mycoliza/status/1298399240121544705
        collections::LinkedList,
        // A Future is the fundamental block of any async executor. It is a trait
        // that types can make or an unnameable type that an async function can
        // make. We say it's unnameable because you don't actually define the type
        // anywhere and just like a closure you can only specify its behavior with
        // a trait. You can't give it a name like you would when you do something
        // like `pub struct Foo;`. These types, whether nameable or not, represent all
        // the state needed to have an asynchronous function. You poll the future to
        // drive its computation along like a state machine that makes transistions
        // from one state to another till it finishes. If you reach a point where it
        // would yield execution, then it needs to be rescheduled to be polled again
        // in the future. It yields though so that you can drive other futures
        // forward in their computation!
        //
        // This is the important part to understand here with the executor: the
        // Future trait defines the API we use to drive forward computation of it,
        // while the implementor of the trait defines how that computation will work
        // and when to yield to the executor. You'll see later that we have an
        // example of writing a `Sleep` future by hand as well as unnameable async
        // code using `async { }` and we'll expand on when those yield and what it
        // desugars to in practice. We're here to demystify the mystical magic of
        // async code.
        future::Future,
        // Ah Pin. What a confusing type. The best way to think about `Pin` is that
        // it records when a value became immovable or pinned in place. `Pin` doesn't
        // actually pin the value, it just notes that the value will not move, much
        // in the same way that you can specify Rust lifetimes. It only records what
        // the lifetime already is, it doesn't actually create said lifetime! At the
        // bottom of this, I've linked some more in depth reading on Pin, but if you
        // don't know much about Pin, starting with the standard library docs isn't a
        // bad place.
        //
        // Note: Unpin is also a confusing name and if you think of it as
        // MaybePinned you'll have a better time as the value may be pinned or it
        // may not be pinned. It just marks that if you have a Pinned value and it
        // moves that's okay and it's safe to do so, whereas for types that do not
        // implement Unpin and they somehow move, will cause some really bad things
        // to happen since it's not safe for the type to be moved after being
        // pinned. We create our executor with the assumption that every future we
        // get will need to be a pinned value, even if it is actually Unpin. This
        // makes it nicer for everyone using the executor as it's very easy to make
        // types that do not implement Unpin.
        pin::Pin,
        sync::{
            // What's not to love about Atomics? This lets us have thread safe
            // access to primitives so that we can modify them or load them using
            // Ordering to tell the compiler how it should handle giving out access
            // to the data. Atomics are a rather deep topic that's out of scope for
            // this. Just note that we want to change a usize safely across threads!
            atomic::{AtomicUsize, Ordering},
            // Arc is probably one of the more important types we'll use in the
            // executor. It lets us freely clone cheap references to the data which
            // we can use across threads while making it easy to not have to worry about
            // complicated lifetimes since we can easily own the data with a call to
            // clone. It's one of my favorite types in the standard library.
            Arc,
            // Normally I would use `parking_lot` for a Mutex, but the goal is to
            // use stdlib only. A personal gripe is that it cares about Mutex
            // poisoning (when a thread panics with a hold on the lock), which is
            // not something I've in practice run into (others might!) and so calling
            // `lock().unwrap()` everywhere can get a bit tedious. That being said
            // Mutexes are great. You make sure only one thing has access to the data
            // at any given time to access or change it.
            Mutex,
        },
        // The task module contains all of the types and traits related to
        // having an executor that can create and run tasks that are `Futures`
        // that need to be polled.
        task::{
            // `Context` is passed in every call to `poll` for a `Future`. We
            // didn't use it in our `Sleep` one, but it has to be passed in
            // regardless. It gives us access to the `Waker` for the future so
            // that we can call it ourselves inside the future if need be!
            Context,
            // Poll is the enum returned from when we poll a `Future`. When we
            // call `poll`, this drives the `Future` forward until it either
            // yields or it returns a value. `Poll` represents that. It is
            // either `Poll::Pending` or `Poll::Ready(T)`. We use this to
            // determine if a `Future` is done or not and if not, then we should
            // keep polling it.
            Poll,
            // This is a trait to define how something in an executor is woken
            // up. We implement it for `Task` which is what lets us create a
            // `Waker` from it, to then make a `Context` which can then be
            // passed into the call to `poll` on the `Future` inside the `Task`.
            Wake,
            // A `Waker` is the type that has a handle to the runtime to let it
            // know when a task is ready to be scheduled for polling. We're
            // doing a very simple version where as soon as a `Task` is done
            // polling we tell the executor to wake it. Instead what you might
            // want to do when creating a `Future` is have a more involved way
            // to only wake when it would be ready to poll, such as a timer
            // completing, or listening for some kind of signal from the OS.
            // It's kind of up to the executor how it wants to do it. Maybe how
            // it schedules things is different or it has special behavior for
            // certain `Future`s that it ships with it. The key thing to note
            // here is that this is how tasks are supposed to be rescheduled for
            // polling.
            Waker,
        },
    };

    /// This is it, the thing we've been alluding to for most of this file. It's
    /// the `Runtime`! What is it? What does it do? Well the `Runtime` is what
    /// actually drives our async code to completion. Remember asynchronous code
    /// is just code that gets run for a bit, yields part way through the
    /// function, then continues when polled and it repeats this process till
    /// being completed. In reality what this means is that the code is run
    /// using synchronous functions that drive tasks in a concurrent manner.
    /// They could also be run concurrently and/or in parallel if the executor
    /// is multithreaded. Tokio is a good example of this model where it runs
    /// tasks in parallel on separate threads and if it has more tasks than
    /// threads, it runs them concurrently on those threads.
    ///
    /// 这就是我们一直在谈论的东西。它是 `Runtime`！
    /// 它是什么？它能做什么？好吧，`Runtime` 就是驱动我们的异步代码执行完成的东西！
    /// 还记得异步代码吗？它只是一些代码，它会运行一段时间，然后通过Sleep函数暂停一段时间，
    /// 然后在轮询时继续运行，然后重复这个过程，直到完成。
    /// 实际上，这意味着代码是使用同步函数运行的，这些函数以并发方式驱动任务。
    /// 如果执行器是多线程的，它们也可以并发运行 和/或 并行运行。
    /// Tokio 就是这种模型的一个很好的例子，它在不同的线程上并行运行任务，
    /// 如果任务数多于线程数，它会在这些线程上并发运行它们。
    ///
    /// Our `Runtime` in particular has:
    pub(crate) struct Runtime {
        /// A queue to place all of the tasks that are spawned on the runtime.
        /// 一个队列，用于放置在运行时上生成的所有任务。
        queue: Queue,
        /// A `Spawner` which can spawn tasks onto our queue for us easily and
        /// lets us call `spawn` and `block_on` with ease.
        /// 一个 `Spawner`，它可以轻松地将任务放入我们的队列中，让我们可以轻松地调用 `spawn` 和 `block_on`。
        spawner: Spawner,
        /// A counter for how many Tasks are on the runtime. We use this in
        /// conjunction with `wait` to block until there are no more tasks on
        /// the executor.
        ///
        tasks: AtomicUsize,
    }

    /// Our runtime type is designed such that we only ever have one running.
    /// You might want to have multiple running in production code though. For
    /// instance you limit what happens on one runtime for a free tier version
    /// and let the non-free version use as many resources as it can. We
    /// implement 3 functions: `start` to actually get async code running, `get`
    /// so that we can get references to the runtime, and `spawner` a
    /// convenience function to get a `Spawner` to spawn tasks onto the `Runtime`.
    /// 我们的运行时类型被设计成只有一个运行时。但是在生产代码中，您可能希望有多个运行时。
    /// 例如，您限制了免费版本上运行时的功能，并让非免费版本可以使用尽可能多的资源。
    /// 我们实现了3个函数：
    /// `start` 来实际运行异步代码。
    /// `get` 以便我们可以获取对运行时的引用。
    /// `spawner` 一个方便的函数，获取一个 `Spawner` 来将任务放入 `Runtime`。
    impl Runtime {
        /// This is what actually drives all of our async code. We spawn a
        /// separate thread that loops getting the next task off the queue and
        /// if it exists polls it or continues if not. It also checks if the
        /// task should block and if it does it just keeps polling the task
        /// until it completes! Otherwise it wakes the task to put it back in
        /// the queue in the non-blocking version if it's still pending.
        /// Otherwise it drops the task by not putting it back into the queue
        /// since it's completed.
        /// 这就是实际驱动我们所有异步代码的程序。
        /// 我们在一个单独的线程中启动一个循环，从队列中获取下一个任务，
        /// 如果存在，则poll它，如果不存在，则继续循环获取任务。
        /// 获取任务后，检查任务是否应该阻塞，如果是，则只会持续poll该任务，直到任务完成！
        /// 否则，它会poll一次任务，如果任务仍然未完成，则以非阻塞的方式将其放回队列中。
        fn start() {
            std::thread::spawn(|| {
                loop {
                    let task = match Runtime::get().queue.lock().unwrap().pop_front() {
                        Some(task) => task,
                        None => continue,
                    };
                    if task.will_block() {
                        while let Poll::Pending = task.poll() {
                            if SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap()
                                .as_micros()
                                % 1000000
                                == 1
                            {
                                // println!("blocking {} {}", current_thread_id(), current_time());
                            }
                        }
                    } else {
                        if let Poll::Pending = task.poll() {
                            if SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap()
                                .as_micros()
                                % 1000000
                                == 1
                            {
                                // println!("waking {} {}", current_thread_id(), current_time());
                            }
                            task.wake();
                        }
                    }
                }
            });
        }

        /// A function to get a reference to the `Runtime`
        /// 一个获取 `Runtime` 引用的函数
        pub(crate) fn get() -> &'static Runtime {
            RUNTIME.get_or_init(setup_runtime)
        }

        /// A function to get a new `Spawner` from the `Runtime`
        /// 一个从 `Runtime` 获取新 `Spawner` 的函数
        pub(crate) fn spawner() -> Spawner {
            Runtime::get().spawner.clone()
        }
    }

    /// This is the initialization function for our `RUNTIME` static below. We
    /// make a call to start it up and then return a `Runtime` to be put in the
    /// static value
    /// 这是我们下面的 `RUNTIME` 静态变量的初始化函数。
    /// 我们调用它来启动`RUNTIME`，然后返回一个 `Runtime` 以放入静态值 RUNTIME 中。
    fn setup_runtime() -> Runtime {
        // This is okay to call because any calls to `Runtime::get()` in here will be blocked
        // until we fully initialize the `Lazy` type thanks to the `call_once`
        // function on `Once` which blocks until it finishes initializing.
        // So we start the runtime inside the initialization function, which depends
        // on it being initialized, but it is able to wait until the runtime is
        // actually initialized and so it all just works.
        // 这里调用是可以的，因为在这里调用 `Runtime::get()` 时，由于 `Once` 上的 `call_once` 函数会阻塞，直到它完成初始化，
        // 所以任何对 `Runtime::get()` 的调用都会被阻塞，直到我们完全初始化了 `Lazy` 类型。
        // 所以我们在初始化函数中启动运行时，这取决于它是否已初始化，这里阻塞直至运行时完成初始化，所以一切都能正常工作。
        Runtime::start();
        let queue = Arc::new(Mutex::new(LinkedList::new()));
        Runtime {
            spawner: Spawner {
                queue: queue.clone(),
            },
            queue,
            tasks: AtomicUsize::new(0),
        }
    }

    /// With all of the work we did in `crate::lazy` we can now create our static type to represent
    /// the singular `Runtime` when it is finally initialized by the `setup_runtime` function.
    /// 在 `crate::lazy` 中完成了所有工作后，我们现在可以创建一个静态类型，以表示最终由 `setup_runtime` 函数初始化的单个 `Runtime`。
    static RUNTIME: crate::lazy::Lazy<Runtime> = crate::lazy::Lazy::new();

    // The queue is a single linked list that contains all of the tasks being
    // run on it. We hand out access to it using a Mutex that has an Arc
    // pointing to it so that we can make sure only one thing is touching the
    // queue state at a given time. This isn't the most efficient pattern
    // especially if we wanted to have the runtime be truly multi-threaded, but
    // for the purposes of the code this works just fine.
    // 队列是一个单链表，其中包含在其上运行的所有任务。
    // 我们使用一个带有指向它的 Arc 的 Mutex 来访问它，以便我们可以确保在给定时间只有一个「事物」能够获取队列状态。
    // 这种模式不是最高效的，特别是如果我们想让运行时真正地多线程化，但是对于这段代码来说，这是可以的。
    type Queue = Arc<Mutex<LinkedList<Arc<Task>>>>;

    /// We've talked about the `Spawner` a lot up till this point, but it's
    /// really just a light wrapper around the queue that knows how to push
    /// tasks onto the queue and create new ones.
    /// 我们一直在讨论 `Spawner`，但它实际上只是一个轻量级的包装器，它知道如何将任务推送到队列中并创建新任务。
    #[derive(Clone)]
    pub(crate) struct Spawner {
        queue: Queue,
    }

    impl Spawner {
        /// This is the function that gets called by the `spawn` function to
        /// actually create a new `Task` in our queue. It takes the `Future`,
        /// constructs a `Task` and then pushes it to the back of the queue.
        /// 这是 `spawn` 函数，用于在队列中实际创建新的 `Task`。
        /// 它接收 `Future`，构造一个 `Task`，然后将其推送到队列的末尾。
        fn spawn(self, future: impl Future<Output=()> + Send + Sync + 'static) {
            self.inner_spawn(Task::new(false, future));
        }
        /// This is the function that gets called by the `spawn_blocking` function to
        /// actually create a new `Task` in our queue. It takes the `Future`,
        /// constructs a `Task` and then pushes it to the front of the queue
        /// where the runtime will check if it should block and then block until
        /// this future completes.
        /// 这是 `spawn_blocking` 函数，用于在队列中实际创建新的 `Task`。
        /// 它接收 `Future`，构造一个 `Task`，然后将其推送到队列的前端，运行时将检查它是否应该阻塞，然后阻塞直到此 future 完成。
        fn spawn_blocking(self, future: impl Future<Output=()> + Send + Sync + 'static) {
            self.inner_spawn_blocking(Task::new(true, future));
        }
        /// This function just takes a `Task` and pushes it onto the queue. We use this
        /// both for spawning new `Task`s and to push old ones that get woken up
        /// back onto the queue.
        /// 这个函数只是接收一个 `Task` 并将其推送到队列中。
        /// 我们用它来启动新的 `Task`，以及将唤醒的旧任务推送回队列。
        fn inner_spawn(self, task: Arc<Task>) {
            self.queue.lock().unwrap().push_back(task);
        }
        /// This function takes a `Task` and pushes it to the front of the queue
        /// if it is meant to block. We use this both for spawning new blocking
        /// `Task`s and to push old ones that get woken up back onto the queue.
        /// 如果它是用于阻塞的，则此函数将 `Task` 推送到队列的前端。
        /// 我们用它来启动新的阻塞 `Task`，以及将唤醒的旧任务推送回队列。
        fn inner_spawn_blocking(self, task: Arc<Task>) {
            self.queue.lock().unwrap().push_front(task);
        }
    }

    /// Spawn a non-blocking `Future` onto the `whorl` runtime
    /// 将非阻塞的 `Future` 放入 `whorl` 运行时
    pub fn spawn(future: impl Future<Output=()> + Send + Sync + 'static) {
        Runtime::spawner().spawn(future);
    }

    /// Block on a `Future` and stop others on the `whorl` runtime until this
    /// one completes.
    /// 阻塞 `Future`，并在 `whorl` 运行时停止其他任务，直到此任务完成。
    pub fn block_on(future: impl Future<Output=()> + Send + Sync + 'static) {
        // println!("block on called {} {}", current_thread_id(), current_time());
        Runtime::spawner().spawn_blocking(future);
    }

    /// Block further execution of a program until all of the tasks on the
    /// `whorl` runtime are completed.
    /// 阻止程序的进一步执行，直到 `whorl` 运行时上的所有任务完成。
    pub fn wait() {
        // println!("wait called {} {}", current_thread_id(), current_time());
        let runtime = Runtime::get();
        while runtime.tasks.load(Ordering::Relaxed) > 0 {}
    }

    /// The `Task` is the basic unit for the executor. It represents a `Future`
    /// that may or may not be completed. We spawn `Task`s to be run and poll
    /// them until completion in a non-blocking manner unless specifically asked
    /// for.
    /// `Task` 是执行器的基本单元。它表示一个可能已完成或未完成的 `Future`。
    /// 我们启动 `Task` 来运行，并以非阻塞方式轮询它们，直到完成，除非明确制定为阻塞任务。
    struct Task {
        /// This is the actual `Future` we will poll inside of a `Task`. We `Box`
        /// and `Pin` the `Future` when we create a task so that we don't need
        /// to worry about pinning or more complicated things in the runtime. We
        /// also need to make sure this is `Send + Sync` so we can use it across threads
        /// and so we lock the `Pin<Box<dyn Future>>` inside a `Mutex`.
        future: Mutex<Pin<Box<dyn Future<Output=()> + Send + Sync + 'static>>>,
        /// We need a way to check if the runtime should block on this task and
        /// so we use a boolean here to check that!
        block: bool,
    }

    impl Task {
        /// This constructs a new task by increasing the count in the runtime of
        /// how many tasks there are, pinning the `Future`, and wrapping it all
        /// in an `Arc`.
        /// 构造新任务，并增加运行时中的任务数量，pinning `Future`，并将其包装在 `Arc` 中。
        fn new(block: bool, future: impl Future<Output=()> + Send + Sync + 'static) -> Arc<Self> {
            Runtime::get().tasks.fetch_add(1, Ordering::Relaxed);
            Arc::new(Task {
                future: Mutex::new(Box::pin(future)),
                block,
            })
        }

        /// We want to use the `Task` itself as a `Waker` which we'll get more
        /// into below. This is a convenience method to construct a new `Waker`.
        /// A neat thing to note for `poll` and here as well is that we can
        /// restrict a method such that it will only work when `self` is a
        /// certain type. In this case you can only call `waker` if the type is
        /// a `&Arc<Task>`. If it was just `Task` it would not compile or work.
        /// 我们希望将 `Task` 本身用作 `Waker`，我们将在下面进一步讨论。
        /// 这是一个方便的方法来构造一个新的 `Waker`。
        /// 有趣的是，对于 `poll` 和这里，我们可以限制一个方法，使其仅在 `self` 是某种类型时才有效。
        /// 在这种情况下，只有当类型是 `&Arc<Task>` 时，才能调用 `waker`。
        fn waker(self: &Arc<Self>) -> Waker {
            self.clone().into()
        }

        /// This is a convenience method to `poll` a `Future` by creating the
        /// `Waker` and `Context` and then getting access to the actual `Future`
        /// inside the `Mutex` and calling `poll` on that.
        /// 这是一个方便的方法来 `poll` `Future`，通过创建 `Waker` 和 `Context`，
        /// 然后获取 `Mutex` 内部的实际 `Future` 的访问权限，并对其调用 `poll`。
        fn poll(self: &Arc<Self>) -> Poll<()> {
            let waker = self.waker();
            let mut ctx = Context::from_waker(&waker);
            self.future.lock().unwrap().as_mut().poll(&mut ctx)
        }

        /// Checks the `block` field to see if the `Task` is blocking.
        /// 检查 `block` 字段，以查看 `Task` 是否阻塞。
        fn will_block(&self) -> bool {
            self.block
        }
    }

    /// Since we increase the count everytime we create a new task we also need
    /// to make sure that it *also* decreases the count every time it goes out
    /// of scope. This implementation of `Drop` does just that so that we don't
    /// need to bookeep about when and where to subtract from the count.
    /// 由于我们每次创建新任务时都会增加计数，因此我们还需要确保它在每次移出范围时都会减少计数。
    /// 实现 `Drop` 可以实现上面功能，因此我们不需要在何时何地减去计数时进行对账。
    impl Drop for Task {
        fn drop(&mut self) {
            Runtime::get().tasks.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// `Wake` is the crux of all of this executor as it's what lets us
    /// reschedule a task when it's ready to be polled. For our implementation
    /// we do a simple check to see if the task blocks or not and then spawn it back
    /// onto the executor in an appropriate manner.
    /// `Wake` 是这个执行器的关键，因为它使我们能够在任务准备好被poll时重新安排任务。
    /// 对于我们的实现，我们进行了一个简单的检查，以查看任务是否阻塞，然后以适当的方式将其重新放回执行器。
    impl Wake for Task {
        fn wake(self: Arc<Self>) {
            if self.will_block() {
                Runtime::spawner().inner_spawn_blocking(self);
            } else {
                Runtime::spawner().inner_spawn(self);
            }
        }
    }
}

// That's it! A full asynchronous runtime with comments all in less than 1000
// lines. Most of that being the actual comments themselves. I hope this made
// how Rust async executors work less magical and more understandable. It's a
// lot to take in, but at the end of the day it's just keeping track of state
// and a couple of loops to get it all working. If you want to see how to write
// a more performant executor that's being used in production and works really
// well, then consider reading the source code for `tokio`. I myself learned
// quite a bit reading it and it's fascinating and fairly well documented.
// If you're interested in learning even more about async Rust or you want to
// learn more in-depth things about it, then I recommend reading this list
// of resources and articles I've found useful that are worth your time:
// 这就是它了！一个完整的异步运行时，注释及代码全部不到1000行。
// 大部分实际都是注释。我希望这使得 Rust 异步执行器的工作方式不再神奇，而是更易于理解。
// 注释很多，但最终只是跟踪状态和几个循环就可以让程序工作。
// 如果你想看看如何编写一个在生产中使用的更高性能的执行器，那么请考虑阅读 `tokio` 的源代码。
// 我自己在阅读它时学到了很多，它很有趣，而且做了相当详细的记录。
// 如果你对学习更多关于 Rust 异步或你想要了解更深入的东西感兴趣，那么我建议你阅读这个我认为有用的资源列表和文章，值得你的时间：

// - Asynchronous Programming in Rust: https://rust-lang.github.io/async-book/01_getting_started/01_chapter.html
// - The Tokio Book: https://tokio.rs/tokio/tutorial
// - Getting in and out of trouble with Rust futures: https://fasterthanli.me/articles/getting-in-and-out-of-trouble-with-rust-futures
// - Pin and Suffering: https://fasterthanli.me/articles/pin-and-suffering
// - Understanding Rust futures by going way too deep: https://fasterthanli.me/articles/understanding-rust-futures-by-going-way-too-deep
// - How Rust optimizes async/await
//   - Part 1: https://tmandry.gitlab.io/blog/posts/optimizing-await-1/
//   - Part 2: https://tmandry.gitlab.io/blog/posts/optimizing-await-2/
// - The standard library docs have even more information and are worth reading.
//   Below are the modules that contain all the types and traits necessary to
//   actually create and run async code. They're fairly in-depth and sometimes
//   require reading other parts to understand a specific part in a really weird
//   dependency graph of sorts, but armed with the knowledge of this executor it
//   should be a bit easier to grok what it all means!
// - 标准库文档中包含更多信息，值得阅读。下面是包含所有必要类型和特征的模块，以实际创建和运行异步代码。
//   它们相当深入，有时需要阅读其他部分才能理解某个特定部分，这真是个奇怪的依赖顺序，
//   但是有了这个执行器的知识，应该会更容易理解它们的含义！
//   - task module: https://doc.rust-lang.org/stable/std/task/index.html
//   - pin module: https://doc.rust-lang.org/stable/std/pin/index.html
//   - future module: https://doc.rust-lang.org/stable/std/future/index.html
