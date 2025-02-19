use crate::config;
use crate::ActionMap;
use crate::{Error, PlaceholderMap, Result};
use async_trait::async_trait;
extern crate log as log_ext;

mod email;
mod log;
mod process;
mod webhook;
pub use self::log::Log;
pub use email::Email;
pub use process::Process;
pub use webhook::Webhook;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Action: Send + Sync {
    async fn trigger(&self, mut placeholders: PlaceholderMap) -> Result<()>;
}

pub struct ActionBase<T>
where
    T: Action,
{
    name: String,
    timeout: std::time::Duration,
    placeholders: PlaceholderMap,
    action: T,
}

impl<T> ActionBase<T>
where
    T: Action,
{
    pub fn new(
        name: String,
        timeout: std::time::Duration,
        placeholders: PlaceholderMap,
        action: T,
    ) -> Result<Self> {
        if name.is_empty() {
            Err(Error(String::from("'name' cannot be empty.")))
        } else if timeout.is_zero() {
            Err(Error(String::from("'timeout' cannot be 0.")))
        } else {
            Ok(Self {
                name,
                timeout,
                placeholders,
                action,
            })
        }
    }

    fn add_placeholders(&self, placeholders: &mut PlaceholderMap) {
        placeholders.insert(String::from("action_name"), self.name.clone());
        crate::merge_placeholders(placeholders, &self.placeholders);
    }
}

#[async_trait]
impl<T> Action for ActionBase<T>
where
    T: Action,
{
    async fn trigger(&self, mut placeholders: PlaceholderMap) -> Result<()> {
        self.add_placeholders(&mut placeholders);
        if placeholders.contains_key("event_name") {
            log_ext::info!(
                "Action '{}' triggered for report event '{}'.",
                placeholders.get("action_name").unwrap(),
                placeholders.get("event_name").unwrap()
            );
        } else {
            log_ext::info!(
                "Action '{}' triggered for alarm '{}', id '{}' from check '{}'.",
                placeholders.get("action_name").unwrap(),
                placeholders.get("alarm_name").unwrap(),
                placeholders.get("alarm_id").unwrap(),
                placeholders.get("check_name").unwrap()
            );
        }
        let res = tokio::time::timeout(self.timeout, self.action.trigger(placeholders)).await;
        match res {
            Ok(inner) => inner,
            Err(_) => Err(Error(format!(
                "Action '{}' timed out after {} seconds.",
                self.name,
                self.timeout.as_secs()
            ))),
        }
    }
}

struct DisabledAction {}

#[async_trait]
impl Action for DisabledAction {
    async fn trigger(&self, placeholders: PlaceholderMap) -> Result<()> {
        if placeholders.contains_key("event_name") {
            log_ext::debug!(
                "Disabled action '{}' triggered for report event '{}'.",
                placeholders.get("action_name").unwrap(),
                placeholders.get("event_name").unwrap()
            );
        } else {
            log_ext::debug!(
                "Disabled action '{}' triggered for alarm '{}', id '{}' from check '{}'.",
                placeholders.get("action_name").unwrap(),
                placeholders.get("alarm_name").unwrap(),
                placeholders.get("alarm_id").unwrap(),
                placeholders.get("check_name").unwrap()
            );
        }
        Ok(())
    }
}

pub fn from_action_config(action_config: &config::Action) -> Result<std::sync::Arc<dyn Action>> {
    if action_config.disable {
        log_ext::info!(
            "Action {}::'{}' is disabled.",
            action_config.type_,
            action_config.name
        );
        Ok(std::sync::Arc::new(ActionBase::new(
            action_config.name.clone(),
            std::time::Duration::from_secs(action_config.timeout as u64),
            action_config.placeholders.clone(),
            DisabledAction {},
        )?))
    } else {
        Ok(match &action_config.type_ {
            config::ActionType::Email(_) => std::sync::Arc::new(ActionBase::new(
                action_config.name.clone(),
                std::time::Duration::from_secs(action_config.timeout as u64),
                action_config.placeholders.clone(),
                Email::try_from(action_config)?,
            )?),
            config::ActionType::Log(_) => std::sync::Arc::new(ActionBase::new(
                action_config.name.clone(),
                std::time::Duration::from_secs(action_config.timeout as u64),
                action_config.placeholders.clone(),
                Log::try_from(action_config)?,
            )?),
            config::ActionType::Process(_) => std::sync::Arc::new(ActionBase::new(
                action_config.name.clone(),
                std::time::Duration::from_secs(action_config.timeout as u64),
                action_config.placeholders.clone(),
                Process::try_from(action_config)?,
            )?),
            config::ActionType::Webhook(_) => std::sync::Arc::new(ActionBase::new(
                action_config.name.clone(),
                std::time::Duration::from_secs(action_config.timeout as u64),
                action_config.placeholders.clone(),
                Webhook::try_from(action_config)?,
            )?),
        })
    }
}

pub fn get_action(action: &String, actions: &ActionMap) -> Result<std::sync::Arc<dyn Action>> {
    if action.is_empty() {
        Err(Error(String::from("'name' cannot be empty.")))
    } else {
        Ok(actions
            .get(action)
            .ok_or_else(|| Error(format!("Action '{}' not found.", action)))?
            .clone())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_placeholders() {
        let mut mock_action = MockAction::new();
        mock_action
            .expect_trigger()
            .once()
            .with(eq(PlaceholderMap::from([
                (String::from("action_name"), String::from("Name")),
                (String::from("Hello"), String::from("World")),
                (String::from("Foo"), String::from("Bar")),
            ])))
            .returning(|_| Ok(()));
        let action = ActionBase::new(
            String::from("Name"),
            std::time::Duration::from_secs(1),
            PlaceholderMap::from([(String::from("Hello"), String::from("World"))]),
            mock_action,
        )
        .unwrap();
        action
            .trigger(PlaceholderMap::from([(
                String::from("Foo"),
                String::from("Bar"),
            )]))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_timeout() {
        struct TimeoutMockAction {}
        #[async_trait]
        impl Action for TimeoutMockAction {
            async fn trigger(&self, mut _placeholders: PlaceholderMap) -> Result<()> {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                Ok(())
            }
        }
        let action = ActionBase::new(
            String::from("Name"),
            std::time::Duration::from_secs(1),
            PlaceholderMap::new(),
            TimeoutMockAction {},
        )
        .unwrap();
        assert!(matches!(
            action.trigger(PlaceholderMap::new()).await,
            Err(_)
        ));
    }
}
