@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package com.example.app

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun AppScreen(title: String) {
    var checked by remember { mutableStateOf(false) }
    var text by remember { mutableStateOf("") }
    var slider by remember { mutableStateOf(0.5f) }
    MaterialTheme {
        Scaffold(
            topBar = { TopAppBar(title = { Text(title) }) },
            floatingActionButton = { FloatingActionButton(onClick = { checked = !checked }) { Icon(Icons.Filled.Add, contentDescription = "add") } },
        ) { pad ->
            Column(Modifier.padding(pad).fillMaxSize()) {
                Text("Hello $title", style = MaterialTheme.typography.titleLarge)
                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Checkbox(checked = checked, onCheckedChange = { checked = it })
                    Switch(checked = checked, onCheckedChange = { checked = it })
                    Spacer(Modifier.width(8.dp))
                    Button(onClick = { checked = false }) { Text("Reset") }
                    OutlinedButton(onClick = {}) { Text("Other") }
                }
                Slider(value = slider, onValueChange = { slider = it })
                OutlinedTextField(value = text, onValueChange = { text = it }, label = { Text("Name") })
                HorizontalDivider()
                AnimatedVisibility(visible = checked) {
                    Card(Modifier.padding(8.dp)) { Box(Modifier.padding(16.dp)) { Text("Checked!") } }
                }
                CircularProgressIndicator()
                LazyColumn {
                    items(listOf("a", "b", "c")) { s -> ListItem(headlineContent = { Text(s) }) }
                }
            }
        }
    }
}
