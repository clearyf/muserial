use smol::lock::OnceCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct SendStop {
    c: Rc<OnceCell<()>>,
}

impl SendStop {
    pub fn new() -> SendStop {
        SendStop {
            c: Rc::new(OnceCell::new()),
        }
    }

    pub async fn should_stop(&self) -> () {
        let _ = self.c.wait().await;
    }
}

impl Drop for SendStop {
    fn drop(&mut self) {
        let _ = smol::block_on(self.c.set(()));
    }
}
