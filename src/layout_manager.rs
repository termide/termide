use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::constants::DEFAULT_FM_WIDTH;
use crate::panels::panel_group::PanelGroup;
use crate::panels::Panel;

/// Цель фокуса: либо статический FileManager, либо группа панелей
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    /// Фокус на статическом FileManager (левая колонка)
    FileManager,
    /// Фокус на группе панелей (индекс группы)
    Group(usize),
}

/// Менеджер компоновки панелей с поддержкой аккордеона
pub struct LayoutManager {
    /// Файловый менеджер (всегда отдельно, не участвует в аккордеоне)
    pub file_manager: Option<Box<dyn Panel>>,

    /// Группы панелей (горизонтальные колонки с вертикальным аккордеоном внутри)
    pub panel_groups: Vec<PanelGroup>,

    /// Текущий фокус (FileManager или группа панелей)
    pub focus: FocusTarget,
}

impl LayoutManager {
    /// Создать новый пустой менеджер
    pub fn new() -> Self {
        Self {
            file_manager: None,
            panel_groups: Vec::new(),
            focus: FocusTarget::Group(0),
        }
    }

    /// Добавить файловый менеджер
    pub fn set_file_manager(&mut self, fm: Box<dyn Panel>) {
        self.file_manager = Some(fm);
    }

    /// Получить мутабельную ссылку на файловый менеджер
    pub fn file_manager_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.file_manager.as_mut()
    }

    /// Добавить панель с учётом автоматического стекирования
    pub fn add_panel(&mut self, panel: Box<dyn Panel>, config: &Config, terminal_width: u16) {
        // Рассчитать доступную ширину для панелей
        let fm_width = if self.file_manager.is_some() {
            DEFAULT_FM_WIDTH
        } else {
            0
        };
        let available_width = terminal_width.saturating_sub(fm_width);

        if self.panel_groups.is_empty() {
            // Первая панель - создать новую группу с auto-width
            // Она займет всё доступное пространство при рендеринге
            let group = PanelGroup::new(panel);
            self.panel_groups.push(group);
            self.focus = FocusTarget::Group(0);
            return;
        }

        // Получить индекс активной группы (если фокус на группе)
        let active_group_idx = match self.focus {
            FocusTarget::Group(idx) => idx,
            FocusTarget::FileManager => 0, // Если фокус на FM, добавить в первую группу
        };

        // Вычислить среднюю ширину после добавления новой группы
        // redistribute_widths_proportionally() распределит available_width между всеми группами
        // Используем среднюю ширину как оценку для порогового решения
        let num_groups_after_split = self.panel_groups.len() + 1;
        let new_width_if_split = available_width / num_groups_after_split as u16;

        if new_width_if_split < config.min_panel_width {
            // Автостекирование: добавить в текущую группу вертикально
            if let Some(active_group) = self.panel_groups.get_mut(active_group_idx) {
                active_group.add_panel(panel);
                // Развернуть новую панель
                active_group.set_expanded(active_group.len() - 1);
                // Переключить фокус на эту группу
                self.focus = FocusTarget::Group(active_group_idx);
            }
        } else {
            // Создать новую группу горизонтально
            let new_group = PanelGroup::new(panel);
            self.panel_groups.push(new_group);
            self.focus = FocusTarget::Group(self.panel_groups.len() - 1);
            // Установить фиксированные ширины для всех групп
            self.redistribute_widths_proportionally(available_width);
        }
    }

    /// Toggle panel stacking/unstacking with smart direction choice
    ///
    /// Behavior:
    /// - If current panel is ALONE in its group (len == 1):
    ///   - Priority: merge into LEFT group (if exists)
    ///   - Fallback: merge into RIGHT group (if exists)
    ///   - Error: no groups to merge with
    /// - If current panel is in a group WITH OTHERS (len > 1):
    ///   - Unstack current expanded panel into new group to the RIGHT
    pub fn toggle_panel_stacking(&mut self, available_width: u16) -> Result<()> {
        let active_group_idx = match self.focus {
            FocusTarget::Group(idx) => idx,
            FocusTarget::FileManager => {
                return Err(anyhow!("Cannot stack/unstack FileManager"));
            }
        };

        let group = self
            .panel_groups
            .get(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        let group_len = group.len();

        if group_len == 1 {
            // Single panel in group - merge with adjacent group
            if self.panel_groups.len() == 1 {
                return Err(anyhow!("Only one group exists, nothing to merge with"));
            }

            // Priority: left
            if active_group_idx > 0 {
                self.merge_into_left(active_group_idx, available_width)
            }
            // Fallback: right
            else if active_group_idx + 1 < self.panel_groups.len() {
                self.merge_into_right(active_group_idx, available_width)
            } else {
                Err(anyhow!("No adjacent group found"))
            }
        } else {
            // Multiple panels in group - unstack current panel
            self.unstack_current_panel(active_group_idx, available_width)
        }
    }

    /// Merge current single-panel group into LEFT group
    fn merge_into_left(&mut self, active_group_idx: usize, available_width: u16) -> Result<()> {
        if active_group_idx == 0 {
            return Err(anyhow!("No left group to merge into"));
        }

        // Extract current group
        let current_group = self.panel_groups.remove(active_group_idx);
        let mut panels = current_group.take_panels();

        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        // Add panel to left group
        let left_group_idx = active_group_idx - 1;
        if let Some(left_group) = self.panel_groups.get_mut(left_group_idx) {
            left_group.add_panel(panel);
            left_group.set_expanded(left_group.len() - 1);
        }

        // Update focus to left group
        self.focus = FocusTarget::Group(left_group_idx);

        // Пропорционально перераспределить ширины после удаления группы
        self.redistribute_widths_proportionally(available_width);

        Ok(())
    }

    /// Merge current single-panel group into RIGHT group
    fn merge_into_right(&mut self, active_group_idx: usize, available_width: u16) -> Result<()> {
        if active_group_idx >= self.panel_groups.len().saturating_sub(1) {
            return Err(anyhow!("No right group to merge into"));
        }

        // Extract current group
        let current_group = self.panel_groups.remove(active_group_idx);
        let mut panels = current_group.take_panels();

        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        // After removal, right group shifted left to current index
        if let Some(right_group) = self.panel_groups.get_mut(active_group_idx) {
            right_group.add_panel(panel);
            right_group.set_expanded(right_group.len() - 1);
        }

        // Update focus to right group (now at current index)
        self.focus = FocusTarget::Group(active_group_idx);

        // Пропорционально перераспределить ширины после удаления группы
        self.redistribute_widths_proportionally(available_width);

        Ok(())
    }

    /// Unstack current expanded panel into new group to the RIGHT
    fn unstack_current_panel(
        &mut self,
        active_group_idx: usize,
        available_width: u16,
    ) -> Result<()> {
        let group = self
            .panel_groups
            .get_mut(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        if group.len() <= 1 {
            return Err(anyhow!("Panel is already alone in group"));
        }

        let expanded_idx = group.expanded_index();
        let panel_to_extract = group
            .remove_panel(expanded_idx)
            .ok_or_else(|| anyhow!("No panel to unstack"))?;

        // Create new group to the right
        let new_group = PanelGroup::new(panel_to_extract);
        self.panel_groups.insert(active_group_idx + 1, new_group);

        // Move focus to new group
        self.focus = FocusTarget::Group(active_group_idx + 1);

        // Пропорционально перераспределить ширины всех групп
        self.redistribute_widths_proportionally(available_width);

        Ok(())
    }

    /// Переместить панель в предыдущую группу
    pub fn move_panel_to_prev_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = match self.focus {
            FocusTarget::FileManager => return Err(anyhow!("Cannot move FileManager")),
            FocusTarget::Group(idx) => idx,
        };

        // Игнорировать если уже первая группа
        if group_idx == 0 {
            return Ok(());
        }

        if self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1) {
            // Панель одна в группе → поменять группы местами
            self.panel_groups.swap(group_idx - 1, group_idx);
            self.focus = FocusTarget::Group(group_idx - 1);
        } else {
            // Панель не одна → извлечь и добавить в предыдущую группу
            let group = self.panel_groups.get_mut(group_idx).unwrap();
            let expanded_idx = group.expanded_index();
            let panel = group.remove_panel(expanded_idx).unwrap();

            // Добавить в предыдущую группу
            let prev_group = self.panel_groups.get_mut(group_idx - 1).unwrap();
            prev_group.add_panel(panel);
            prev_group.set_expanded(prev_group.len() - 1);

            self.focus = FocusTarget::Group(group_idx - 1);

            // Удалить пустую группу если осталась
            if self
                .panel_groups
                .get(group_idx)
                .map(|g| g.is_empty())
                .unwrap_or(false)
            {
                self.panel_groups.remove(group_idx);
                // Пропорционально перераспределить ширины после удаления группы
                self.redistribute_widths_proportionally(available_width);
            }
        }

        Ok(())
    }

    /// Переместить панель в следующую группу
    pub fn move_panel_to_next_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = match self.focus {
            FocusTarget::FileManager => return Err(anyhow!("Cannot move FileManager")),
            FocusTarget::Group(idx) => idx,
        };

        // Игнорировать если уже последняя группа
        if group_idx >= self.panel_groups.len().saturating_sub(1) {
            return Ok(());
        }

        if self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1) {
            // Панель одна в группе → поменять группы местами
            self.panel_groups.swap(group_idx, group_idx + 1);
            self.focus = FocusTarget::Group(group_idx + 1);
        } else {
            // Панель не одна → извлечь и добавить в следующую группу
            let group = self.panel_groups.get_mut(group_idx).unwrap();
            let expanded_idx = group.expanded_index();
            let panel = group.remove_panel(expanded_idx).unwrap();

            // Добавить в следующую группу
            let next_group = self.panel_groups.get_mut(group_idx + 1).unwrap();
            next_group.add_panel(panel);
            next_group.set_expanded(next_group.len() - 1);

            self.focus = FocusTarget::Group(group_idx + 1);

            // Удалить пустую группу если осталась
            if self
                .panel_groups
                .get(group_idx)
                .map(|g| g.is_empty())
                .unwrap_or(false)
            {
                self.panel_groups.remove(group_idx);
                // Скорректировать фокус (группа сдвинулась назад)
                self.focus = FocusTarget::Group(group_idx);
                // Пропорционально перераспределить ширины после удаления группы
                self.redistribute_widths_proportionally(available_width);
            }
        }

        Ok(())
    }

    /// Переместить панель в первую группу
    pub fn move_panel_to_first_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = match self.focus {
            FocusTarget::FileManager => return Err(anyhow!("Cannot move FileManager")),
            FocusTarget::Group(idx) => idx,
        };

        // Уже в первой группе
        if group_idx == 0 {
            return Ok(());
        }

        // Извлечь панель из текущей группы
        let is_alone = self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1);
        let group = self.panel_groups.get_mut(group_idx).unwrap();
        let expanded_idx = group.expanded_index();
        let panel = group.remove_panel(expanded_idx).unwrap();

        // Добавить в первую группу
        self.panel_groups.get_mut(0).unwrap().add_panel(panel);
        let target_len = self.panel_groups.first().unwrap().len();
        self.panel_groups
            .get_mut(0)
            .unwrap()
            .set_expanded(target_len - 1);

        self.focus = FocusTarget::Group(0);

        // Удалить пустую группу если панель была одна
        if is_alone {
            self.panel_groups.remove(group_idx);
            // Пропорционально перераспределить ширины после удаления группы
            self.redistribute_widths_proportionally(available_width);
        }

        Ok(())
    }

    /// Переместить панель в последнюю группу
    pub fn move_panel_to_last_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = match self.focus {
            FocusTarget::FileManager => return Err(anyhow!("Cannot move FileManager")),
            FocusTarget::Group(idx) => idx,
        };

        let last_idx = self.panel_groups.len().saturating_sub(1);

        // Уже в последней группе
        if group_idx == last_idx {
            return Ok(());
        }

        // Извлечь панель из текущей группы
        let is_alone = self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1);
        let group = self.panel_groups.get_mut(group_idx).unwrap();
        let expanded_idx = group.expanded_index();
        let panel = group.remove_panel(expanded_idx).unwrap();

        // Добавить в последнюю группу
        self.panel_groups
            .get_mut(last_idx)
            .unwrap()
            .add_panel(panel);
        let target_len = self.panel_groups.get(last_idx).unwrap().len();
        self.panel_groups
            .get_mut(last_idx)
            .unwrap()
            .set_expanded(target_len - 1);

        // Удалить пустую группу если панель была одна
        if is_alone {
            self.panel_groups.remove(group_idx);
            // last_idx shifted back, so focus stays the same value
            // Пропорционально перераспределить ширины после удаления группы
            self.redistribute_widths_proportionally(available_width);
        }

        self.focus = FocusTarget::Group(self.panel_groups.len().saturating_sub(1));

        Ok(())
    }

    /// Переключиться на следующую группу (горизонтально)
    /// Порядок: FileManager → Group(0) → Group(1) → ... → Group(0) ...
    pub fn next_group(&mut self) {
        match self.focus {
            FocusTarget::FileManager => {
                // С FM переключиться на первую группу, если есть
                if !self.panel_groups.is_empty() {
                    self.focus = FocusTarget::Group(0);
                }
            }
            FocusTarget::Group(idx) => {
                if !self.panel_groups.is_empty() {
                    let next_idx = (idx + 1) % self.panel_groups.len();
                    self.focus = FocusTarget::Group(next_idx);
                }
            }
        }
    }

    /// Переключиться на предыдущую группу (горизонтально)
    /// Порядок: ... Group(1) → Group(0) → FileManager (если есть) → Group(последняя) ...
    pub fn prev_group(&mut self) {
        match self.focus {
            FocusTarget::FileManager => {
                // С FM остаться на FM (это крайняя левая позиция)
            }
            FocusTarget::Group(idx) => {
                if idx == 0 {
                    // С первой группы переключиться на FM, если он есть
                    if self.file_manager.is_some() {
                        self.focus = FocusTarget::FileManager;
                    } else if !self.panel_groups.is_empty() {
                        // Если FM нет, перейти к последней группе (цикл)
                        self.focus = FocusTarget::Group(self.panel_groups.len() - 1);
                    }
                } else if !self.panel_groups.is_empty() {
                    self.focus = FocusTarget::Group(idx - 1);
                }
            }
        }
    }

    /// Переключиться на следующую панель в текущей группе (вертикально)
    /// Не работает если фокус на FileManager
    pub fn next_panel_in_group(&mut self) {
        if let FocusTarget::Group(idx) = self.focus {
            if let Some(group) = self.panel_groups.get_mut(idx) {
                group.next_panel();
            }
        }
    }

    /// Переключиться на предыдущую панель в текущей группе (вертикально)
    /// Не работает если фокус на FileManager
    pub fn prev_panel_in_group(&mut self) {
        if let FocusTarget::Group(idx) = self.focus {
            if let Some(group) = self.panel_groups.get_mut(idx) {
                group.prev_panel();
            }
        }
    }

    /// Получить мутабельную ссылку на активную панель
    pub fn active_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        match self.focus {
            FocusTarget::FileManager => self.file_manager.as_mut(),
            FocusTarget::Group(idx) => self
                .panel_groups
                .get_mut(idx)
                .and_then(|group| group.expanded_panel_mut()),
        }
    }

    /// Получить индекс активной группы
    /// Возвращает None если фокус на FileManager
    pub fn active_group_index(&self) -> Option<usize> {
        match self.focus {
            FocusTarget::FileManager => None,
            FocusTarget::Group(idx) => Some(idx),
        }
    }

    /// Итератор по всем панелям (мутабельный)
    /// Возвращает FM (если есть), затем все панели во всех группах
    pub fn iter_all_panels_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Panel>> {
        self.file_manager.iter_mut().chain(
            self.panel_groups
                .iter_mut()
                .flat_map(|g| g.panels_mut().iter_mut()),
        )
    }

    /// Закрыть активную панель
    pub fn close_active_panel(&mut self, available_width: u16) -> Result<()> {
        let active_group_idx = match self.focus {
            FocusTarget::FileManager => {
                return Err(anyhow!("Cannot close FileManager"));
            }
            FocusTarget::Group(idx) => idx,
        };

        let group = self
            .panel_groups
            .get_mut(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        if group.len() <= 1 {
            // Последняя панель в группе - удалить всю группу
            self.panel_groups.remove(active_group_idx);

            // Скорректировать focus
            if !self.panel_groups.is_empty() {
                if active_group_idx >= self.panel_groups.len() {
                    self.focus = FocusTarget::Group(self.panel_groups.len() - 1);
                } else {
                    self.focus = FocusTarget::Group(active_group_idx);
                }
            } else {
                // Все группы удалены, переключить фокус на FM если он есть
                if self.file_manager.is_some() {
                    self.focus = FocusTarget::FileManager;
                } else {
                    // Нет ни FM, ни групп - установить фокус на Group(0) как начальное состояние
                    self.focus = FocusTarget::Group(0);
                }
            }

            // Пропорционально перераспределить ширины оставшихся групп (группа была удалена)
            self.redistribute_widths_proportionally(available_width);
        } else {
            // Удалить только панель из группы (группа остается с той же шириной)
            let expanded_idx = group.expanded_index();
            group.remove_panel(expanded_idx);
            // НЕ перераспределяем ширины - группа остается с существующей шириной
        }

        Ok(())
    }

    /// Проверить, можно ли закрыть активную панель
    pub fn can_close_active(&self) -> bool {
        // Можно закрыть, если есть хотя бы одна группа с панелями
        !self.panel_groups.is_empty()
    }

    /// Проверить, есть ли файловый менеджер
    pub fn has_file_manager(&self) -> bool {
        self.file_manager.is_some()
    }

    /// Вычислить фактические ширины всех групп в текущий момент
    /// Для fixed групп возвращает width, для auto групп вычисляет из оставшегося места
    pub fn calculate_actual_widths(&self, available_width: u16) -> Vec<u16> {
        if self.panel_groups.is_empty() {
            return Vec::new();
        }

        // Собрать fixed ширины
        let total_fixed_width: u16 = self.panel_groups.iter().filter_map(|g| g.width).sum();

        // Посчитать количество auto-групп
        let auto_count = self
            .panel_groups
            .iter()
            .filter(|g| g.width.is_none())
            .count();

        // Вычислить ширину для auto-групп
        let remaining_width = available_width.saturating_sub(total_fixed_width);
        let auto_width = if auto_count > 0 {
            remaining_width / auto_count as u16
        } else {
            0
        };

        // Собрать фактические ширины всех групп
        self.panel_groups
            .iter()
            .map(|g| g.width.unwrap_or(auto_width))
            .collect()
    }

    /// Пропорционально перераспределить ширины групп
    /// Вычисляет фактические текущие ширины (включая auto-группы) и
    /// перераспределяет пространство пропорционально между ВСЕМИ группами.
    /// Все группы получают фиксированную ширину (width = Some(n)).
    pub fn redistribute_widths_proportionally(&mut self, available_width: u16) {
        if self.panel_groups.is_empty() {
            return;
        }

        // Edge case: одна группа занимает всю доступную ширину
        if self.panel_groups.len() == 1 {
            self.panel_groups[0].width = Some(available_width.max(20));
            return;
        }

        // Заморозить auto-width группы: установить им ширины
        // Специальная логика для добавления новых групп к существующим
        let has_auto_groups = self.panel_groups.iter().any(|g| g.width.is_none());
        if has_auto_groups {
            let auto_count = self
                .panel_groups
                .iter()
                .filter(|g| g.width.is_none())
                .count();
            let fixed_groups: Vec<u16> = self.panel_groups.iter().filter_map(|g| g.width).collect();

            if !fixed_groups.is_empty() && auto_count > 0 {
                // Есть и fixed и auto группы - новая группа добавляется к существующим
                // Рассчитать среднюю ширину существующих fixed групп
                let avg_fixed_width: u16 =
                    fixed_groups.iter().sum::<u16>() / fixed_groups.len() as u16;

                // Установить новым auto-группам среднюю ширину существующих
                for group in self.panel_groups.iter_mut() {
                    if group.width.is_none() {
                        group.width = Some(avg_fixed_width.max(20));
                    }
                }
            } else {
                // Все группы auto - первое открытие (50/50 для двух групп)
                // Использовать фактические ширины из рендеринга
                let actual_widths_before_freeze = self.calculate_actual_widths(available_width);
                for (idx, &width) in actual_widths_before_freeze.iter().enumerate() {
                    if self.panel_groups[idx].width.is_none() {
                        self.panel_groups[idx].width = Some(width.max(20));
                    }
                }
            }
        }

        // Вычислить фактические текущие ширины всех групп (теперь все с fixed width)
        let actual_widths = self.calculate_actual_widths(available_width);

        // Вычислить общую сумму фактических ширин
        let total_actual: u16 = actual_widths.iter().sum();

        if total_actual == 0 {
            return; // Защита от деления на ноль
        }

        // Перераспределить пропорционально для ВСЕХ групп
        // Последняя группа получает остаток для точности
        let mut new_widths = Vec::with_capacity(actual_widths.len());
        let mut allocated_width: u16 = 0;

        for (idx, &actual_width) in actual_widths.iter().enumerate() {
            let is_last = idx == actual_widths.len() - 1;

            let width = if is_last {
                // Последняя группа получает весь остаток
                available_width.saturating_sub(allocated_width).max(20)
            } else {
                // Для остальных: округлить пропорционально
                let proportion = actual_width as f64 / total_actual as f64;
                let w = (available_width as f64 * proportion).round() as u16;
                let w = w.max(20);
                allocated_width = allocated_width.saturating_add(w);
                w
            };

            new_widths.push(width);
        }

        // Применить новые ширины ко всем группам (все становятся фиксированными)
        for (idx, &width) in new_widths.iter().enumerate() {
            self.panel_groups[idx].width = Some(width);
        }
    }
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::new()
    }
}
