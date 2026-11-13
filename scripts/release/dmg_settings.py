"""dmgbuild settings: writes Finder metadata without launching Finder."""
import os
application = os.environ['DLM_PACKAGE_APP']
files = [application]
symlinks = {'Applications': '/Applications'}
format = 'UDZO'
filesystem = 'HFS+'
background = os.environ['DLM_PACKAGE_BACKGROUND']
window_rect = ((100, 100), (660, 420))
icon_locations = {'DLM.app': (170, 190), 'Applications': (490, 190)}
icon_size = 112
text_size = 14
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
arrange_by = None
default_view = 'icon-view'
include_icon_view_settings = True
include_list_view_settings = False
